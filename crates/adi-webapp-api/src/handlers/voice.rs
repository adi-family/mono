//! `/api/voice` — dictation: audio recorded in the panel, words back.
//!
//! **Why the server is in this at all.** The browser can recognise speech by itself, and for the
//! `browser` engine it does — nothing here runs. The rest are HTTP services behind an API key,
//! and a key that reached the page would be a key handed to every script on it. So the panel
//! uploads the clip and this module holds the credential, which also means the four providers'
//! disagreements are settled once, here, instead of in wasm:
//!
//! - <https://platform.openai.com/docs/api-reference/audio/createTranscription>
//! - <https://console.groq.com/docs/speech-to-text>
//! - <https://elevenlabs.io/docs/api-reference/speech-to-text/convert>
//! - <https://developers.deepgram.com/reference/listen-file>
//!
//! Three of them take `multipart/form-data` and name the model field differently; Deepgram takes
//! the audio bytes raw and asks for the model in the query string. All four bury the text at a
//! different depth of the response JSON.
//!
//! The audio arrives as a raw body rather than a form, because the panel already has the bytes
//! and a `Content-Type` from `MediaRecorder` and has nothing to add to them.

use std::time::Duration;

use adi_secrets::Secrets;
use serde_json::Value;

use crate::types::{Transcript, VoiceEngineDto, VoiceState};

use super::response::{Response, error, ok_json};

/// Long enough for a minute of speech over a slow uplink, short enough that a wedged provider
/// frees the blocking-pool thread it is sitting on. Dictation is interactive: a caller who has
/// waited this long has already given up and pressed the button again.
const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Refuse a clip bigger than this before spending a request on it. Roughly ten minutes of Opus —
/// far past a dictated message, and well under the 25 MiB the strictest provider here accepts, so
/// the rejection comes from us with a readable reason rather than from them with a 413.
const MAX_CLIP: usize = 8 << 20;

/// How a provider wants the audio handed to it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wire {
    /// `multipart/form-data`, audio under `file`, model under the named field.
    Multipart { model_field: &'static str },
    /// The bytes as the whole body, model in the query string.
    Raw,
}

/// One speech-to-text service: everything that differs between them, in one place.
struct Engine {
    id: &'static str,
    label: &'static str,
    /// Tried in order; the first that resolves wins. Both a stored secret and a plain environment
    /// variable count, so a key already exported for the agent loop works here without being
    /// entered twice.
    key_names: &'static [&'static str],
    url: &'static str,
    model: &'static str,
    wire: Wire,
    /// Builds the auth header. A pair rather than a bearer string because `ElevenLabs` uses its own
    /// header name and Deepgram its own scheme.
    auth: fn(&str) -> (String, String),
    /// Where the text sits in the response. Walked key by key; the first path that resolves to a
    /// string wins, so a provider that renamed a field stays readable across the rename.
    text_paths: &'static [&'static [&'static str]],
}

fn bearer(key: &str) -> (String, String) {
    ("authorization".into(), format!("Bearer {key}"))
}

/// Every engine the server can call. `browser` is deliberately absent: it never reaches here.
const ENGINES: &[Engine] = &[
    Engine {
        id: "openai",
        label: "OpenAI",
        key_names: &["OPENAI_API_KEY"],
        url: "https://api.openai.com/v1/audio/transcriptions",
        model: "gpt-4o-transcribe",
        wire: Wire::Multipart {
            model_field: "model",
        },
        auth: bearer,
        text_paths: &[&["text"]],
    },
    Engine {
        id: "groq",
        label: "Groq",
        key_names: &["GROQ_API_KEY"],
        url: "https://api.groq.com/openai/v1/audio/transcriptions",
        model: "whisper-large-v3-turbo",
        wire: Wire::Multipart {
            model_field: "model",
        },
        auth: bearer,
        text_paths: &[&["text"]],
    },
    Engine {
        id: "elevenlabs",
        label: "ElevenLabs",
        key_names: &["ELEVENLABS_API_KEY", "ELEVEN_API_KEY"],
        url: "https://api.elevenlabs.io/v1/speech-to-text",
        model: "scribe_v1",
        // The one that names the model `model_id` rather than `model`; sending `model` is not an
        // error there, it is simply ignored, and the request fails as if none was given.
        wire: Wire::Multipart {
            model_field: "model_id",
        },
        auth: |key| ("xi-api-key".into(), key.to_string()),
        text_paths: &[&["text"]],
    },
    Engine {
        id: "deepgram",
        label: "Deepgram",
        key_names: &["DEEPGRAM_API_KEY"],
        url: "https://api.deepgram.com/v1/listen",
        model: "nova-3",
        wire: Wire::Raw,
        auth: |key| ("authorization".into(), format!("Token {key}")),
        text_paths: &[&[
            "results",
            "channels",
            "0",
            "alternatives",
            "0",
            "transcript",
        ]],
    },
];

/// The id the panel sends for "recognise it yourself".
pub const BROWSER_ENGINE: &str = "browser";

/// A key for this engine, from the secret store first and the process environment second.
///
/// Reading the environment matters on this machine specifically: the agent loop takes provider
/// keys from env vars, so the common case is that `OPENAI_API_KEY` is already exported and
/// dictation should just work rather than demand the same value be stored again.
fn resolve_key(store: &Secrets, engine: &Engine) -> Option<String> {
    for name in engine.key_names {
        if let Ok(Some(value)) = store.reveal(None, name)
            && !value.trim().is_empty()
        {
            return Some(value);
        }
        if let Ok(value) = std::env::var(name)
            && !value.trim().is_empty()
        {
            return Some(value);
        }
    }
    None
}

/// `GET /api/voice` — the engines, and which of them are usable right now.
#[must_use]
pub fn voice(store: &Secrets) -> Response {
    let mut engines = vec![VoiceEngineDto {
        id: BROWSER_ENGINE.to_string(),
        label: "Browser".to_string(),
        ready: true,
        in_browser: true,
        detail: "the browser's own recogniser — no key, no upload".to_string(),
    }];

    // The default is the first *configured* remote engine, in the order declared above, and the
    // browser only if none is: a machine that has paid for a better recogniser should not have to
    // pick it every time, and one that hasn't should still be able to dictate.
    let mut default_engine = BROWSER_ENGINE.to_string();
    for engine in ENGINES {
        let ready = resolve_key(store, engine).is_some();
        if ready && default_engine == BROWSER_ENGINE {
            default_engine = engine.id.to_string();
        }
        engines.push(VoiceEngineDto {
            id: engine.id.to_string(),
            label: engine.label.to_string(),
            ready,
            in_browser: false,
            detail: if ready {
                engine.model.to_string()
            } else {
                format!("set {} to use this", engine.key_names[0])
            },
        });
    }

    ok_json(&VoiceState {
        engines,
        default_engine,
    })
}

/// `POST /api/voice/transcribe?engine=<id>` — a recorded clip in, its words out.
///
/// `content_type` is whatever the browser's `MediaRecorder` chose (`audio/webm;codecs=opus` on
/// Chrome, `audio/mp4` on Safari). It is passed through rather than interpreted: every provider
/// here sniffs the container itself, and the filename extension below exists only because
/// multipart demands *a* name.
#[must_use]
pub fn transcribe(store: &Secrets, engine_id: &str, content_type: &str, audio: &[u8]) -> Response {
    if engine_id == BROWSER_ENGINE {
        return error(
            400,
            "the browser engine recognises speech in the page; it never uploads audio",
        );
    }
    let Some(engine) = ENGINES.iter().find(|e| e.id == engine_id) else {
        return error(400, &format!("unknown speech engine {engine_id:?}"));
    };
    if audio.is_empty() {
        return error(400, "no audio in the request body");
    }
    if audio.len() > MAX_CLIP {
        return error(
            413,
            &format!(
                "that clip is {} MiB; dictation is capped at {} MiB",
                audio.len() >> 20,
                MAX_CLIP >> 20
            ),
        );
    }
    let Some(key) = resolve_key(store, engine) else {
        return error(
            400,
            &format!(
                "no {} key: set {} as a secret (or export it) to dictate through {}",
                engine.label, engine.key_names[0], engine.label
            ),
        );
    };

    match call(engine, &key, content_type, audio) {
        Ok(text) => ok_json(&Transcript {
            text,
            engine: engine.id.to_string(),
        }),
        Err(e) => error(502, &e),
    }
}

/// One request to the provider, and the text dug out of whatever it answered.
fn call(engine: &Engine, key: &str, content_type: &str, audio: &[u8]) -> Result<String, String> {
    // reqwest is built with `rustls-no-provider` (see the workspace manifest), so a client cannot
    // be built until some crypto provider is installed. `install_default` errors only when one
    // already is, which is the outcome wanted.
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let client = reqwest::blocking::Client::builder()
        .timeout(TRANSCRIBE_TIMEOUT)
        .build()
        .map_err(|e| format!("couldn't build the HTTP client: {e}"))?;

    // The raw-bodied provider has nowhere but the query string to put the model, having spent the
    // body on the audio.
    let url = match engine.wire {
        Wire::Multipart { .. } => engine.url.to_string(),
        Wire::Raw => format!("{}?model={}", engine.url, engine.model),
    };
    let (auth_name, auth_value) = (engine.auth)(key);
    let mut req = client.post(url).header(auth_name, auth_value);

    req = match engine.wire {
        Wire::Multipart { model_field } => {
            let part = reqwest::blocking::multipart::Part::bytes(audio.to_vec())
                .file_name(format!("clip.{}", extension(content_type)))
                .mime_str(content_type)
                .map_err(|e| {
                    format!("the recording's content type {content_type:?} is not usable: {e}")
                })?;
            req.multipart(
                reqwest::blocking::multipart::Form::new()
                    .part("file", part)
                    .text(model_field, engine.model),
            )
        }
        Wire::Raw => req
            .header("content-type", content_type)
            .body(audio.to_vec()),
    };

    let resp = req
        .send()
        .map_err(|e| format!("{} could not be reached: {e}", engine.label))?;
    let status = resp.status();
    let body = resp
        .text()
        .map_err(|e| format!("reading {}'s answer failed: {e}", engine.label))?;

    if !status.is_success() {
        // The provider's own message, trimmed of the JSON around it where it has one. What went
        // wrong is nearly always something only they know (a revoked key, a rate limit, a
        // rejected container), so it travels to the user rather than being flattened.
        return Err(format!(
            "{} returned {status}: {}",
            engine.label,
            provider_message(&body)
        ));
    }

    let json: Value = serde_json::from_str(&body)
        .map_err(|e| format!("{} answered something that isn't JSON: {e}", engine.label))?;

    engine
        .text_paths
        .iter()
        .find_map(|path| dig(&json, path))
        .map(|text| text.trim().to_string())
        .ok_or_else(|| {
            format!(
                "{} answered without a transcript: {}",
                engine.label,
                body.chars().take(200).collect::<String>()
            )
        })
}

/// Walk a JSON path, taking a numeric segment as an array index. Returns the string it lands on.
fn dig(value: &Value, path: &[&str]) -> Option<String> {
    let mut node = value;
    for key in path {
        node = match key.parse::<usize>() {
            Ok(index) => node.get(index)?,
            Err(_) => node.get(key)?,
        };
    }
    node.as_str().map(str::to_string)
}

/// The human part of an error body. Providers nest their message differently and some send plain
/// text; anything unrecognised falls back to the body itself, capped so a returned HTML page
/// cannot become the error message.
fn provider_message(body: &str) -> String {
    let trimmed = body.trim();
    if let Ok(json) = serde_json::from_str::<Value>(trimmed) {
        for path in [
            ["error", "message"].as_slice(),
            ["detail", "message"].as_slice(),
            ["error"].as_slice(),
            ["message"].as_slice(),
            ["detail"].as_slice(),
        ] {
            if let Some(found) = dig(&json, path) {
                return found;
            }
        }
    }
    trimmed.chars().take(300).collect()
}

/// A filename extension matching the container, because multipart needs a name and at least one
/// provider reads the extension when the part's MIME type is one it does not know.
fn extension(content_type: &str) -> &'static str {
    let base = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/x-m4a" | "audio/aac" => "m4a",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/ogg" => "ogg",
        _ => "webm",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_engine_never_uploads() {
        let store = Secrets::open();
        let resp = transcribe(&store, BROWSER_ENGINE, "audio/webm", b"xx");
        assert_eq!(resp.status, 400);
        assert!(resp.body.contains("never uploads"), "{}", resp.body);
    }

    #[test]
    fn an_unknown_engine_is_refused_before_any_request() {
        let store = Secrets::open();
        let resp = transcribe(&store, "nope", "audio/webm", b"xx");
        assert_eq!(resp.status, 400);
        assert!(resp.body.contains("unknown speech engine"), "{}", resp.body);
    }

    #[test]
    fn an_empty_clip_is_refused() {
        let store = Secrets::open();
        let resp = transcribe(&store, "openai", "audio/webm", b"");
        assert_eq!(resp.status, 400);
        assert!(resp.body.contains("no audio"), "{}", resp.body);
    }

    #[test]
    fn an_oversized_clip_is_refused_without_a_key_lookup() {
        let store = Secrets::open();
        let big = vec![0u8; MAX_CLIP + 1];
        let resp = transcribe(&store, "openai", "audio/webm", &big);
        assert_eq!(resp.status, 413);
    }

    #[test]
    fn every_engine_is_listed_and_the_browser_is_always_ready() {
        let store = Secrets::open();
        let resp = voice(&store);
        assert_eq!(resp.status, 200);
        let state: VoiceState = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(state.engines.len(), ENGINES.len() + 1);
        let browser = &state.engines[0];
        assert_eq!(browser.id, BROWSER_ENGINE);
        assert!(browser.ready && browser.in_browser);
        // Whatever the machine has configured, the default must name something in the list.
        assert!(state.engines.iter().any(|e| e.id == state.default_engine));
    }

    #[test]
    fn an_unconfigured_engine_says_which_secret_to_set() {
        let store = Secrets::open();
        let resp = voice(&store);
        let state: VoiceState = serde_json::from_str(&resp.body).unwrap();
        for engine in state.engines.iter().filter(|e| !e.ready) {
            assert!(engine.detail.starts_with("set "), "{}", engine.detail);
        }
    }

    #[test]
    fn the_transcript_is_dug_out_of_each_providers_shape() {
        let openai = serde_json::json!({ "text": "hello there" });
        assert_eq!(dig(&openai, &["text"]).unwrap(), "hello there");

        // Deepgram's, which is the one that needs array indices.
        let deepgram = serde_json::json!({
            "results": { "channels": [ { "alternatives": [ { "transcript": "hello there" } ] } ] }
        });
        let path: &[&str] = &[
            "results",
            "channels",
            "0",
            "alternatives",
            "0",
            "transcript",
        ];
        assert_eq!(dig(&deepgram, path).unwrap(), "hello there");
    }

    #[test]
    fn a_provider_error_keeps_the_providers_own_words() {
        let body =
            r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#;
        assert_eq!(provider_message(body), "Incorrect API key provided");
        // ElevenLabs nests it one level deeper, under `detail`.
        assert_eq!(
            provider_message(r#"{"detail":{"message":"quota"}}"#),
            "quota"
        );
        // And anything unparseable still says something rather than nothing.
        assert_eq!(provider_message("  upstream is down  "), "upstream is down");
    }

    #[test]
    fn the_container_decides_the_filename() {
        assert_eq!(extension("audio/webm;codecs=opus"), "webm");
        assert_eq!(extension("audio/mp4"), "m4a");
        assert_eq!(extension("AUDIO/WAV"), "wav");
        // Anything unrecognised is called webm, which is what every browser but Safari records.
        assert_eq!(extension("application/octet-stream"), "webm");
    }
}
