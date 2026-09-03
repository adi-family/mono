//! Dictation: speaking a message instead of typing it.
//!
//! [`mic`] returns a microphone button bound to one composer's text signal. Pressing it opens the
//! microphone; pressing it again closes it and the words land in the box — as text, editable,
//! *unsent*. Dictation writes a draft and never a message: speech recognition is wrong often
//! enough that a transcript posted straight to an agent would be a sentence nobody wrote.
//!
//! **Two routes, one button.** Which one runs is the chosen engine's doing:
//!
//! - `browser` — the page's own `SpeechRecognition`. No audio leaves the machine and words appear
//!   *as they are said*, because the recogniser streams interim guesses.
//! - anything else — record with `MediaRecorder`, upload the clip to `/api/voice/transcribe`,
//!   which holds the API key (see `adi-webapp-api`'s `handlers::voice`). Nothing appears until the
//!   clip is transcribed, but the words are markedly better.
//!
//! **The engine is the user's choice**, taken from the picker beside the button and remembered in
//! `localStorage` — per browser, because which recogniser is worth using depends on the machine
//! you are sitting at, and the server has no business holding that preference.
//!
//! `SpeechRecognition` is reached through `js_sys::Reflect` rather than `web-sys`, which has it
//! only behind the `web_sys_unstable_apis` cfg. Reflection is also what the runtime needs anyway:
//! the constructor is `webkitSpeechRecognition` on Chrome and Safari and `SpeechRecognition` in
//! the specification, so it has to be looked up by name at run time regardless.

use std::cell::RefCell;
use std::rc::Rc;

use adi_webapp_api::types::VoiceEngineDto;
use leptos::html;
use leptos::prelude::LocalStorage;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobEvent, BlobPropertyBag, MediaRecorder, MediaRecorderOptions, MediaStream,
    MediaStreamConstraints,
};

use adi_ui::MicState;

use crate::fetch;

/// Where the engine choice is remembered. Versioned in the key, so a later change to what the
/// value means cannot be read as this one.
const ENGINE_KEY: &str = "adi.voice.engine.v1";

/// The engine id the server uses for "the browser recognises it itself".
const BROWSER: &str = "browser";

/// Containers to record in, best first. Chrome and Firefox take the first; Safari records MP4 and
/// nothing else, and would silently produce an empty clip if handed a type it does not support.
const CONTAINERS: &[&str] = &[
    "audio/webm;codecs=opus",
    "audio/webm",
    "audio/mp4",
    "audio/ogg;codecs=opus",
];

/// A microphone button that dictates into `value`.
///
/// `value` is the composer's own text signal, so a dictated sentence lands where a typed one would
/// and is edited the same way. Text already in the box is kept and spoken words are appended after
/// a space — dictating is adding to the message, not replacing it.
pub fn mic(value: RwSignal<String>) -> impl IntoView {
    let state = RwSignal::new(MicState::Idle);
    let engines = RwSignal::new(Vec::<VoiceEngineDto>::new());
    let engine = RwSignal::new(stored_engine().unwrap_or_else(|| BROWSER.to_string()));
    let picking = RwSignal::new(false);
    // The live session, so a second press can stop what the first started. Stored *locally*: a
    // recogniser and its closures are JS handles, which are neither `Send` nor `Sync`, and the
    // ordinary `StoredValue` demands both.
    let session: StoredValue<Option<Session>, LocalStorage> = StoredValue::new_local(None);

    // What the browser will not do at all is known before any press, and saying so on the button
    // beats a click that appears to work and never produces a word.
    if let Some(why) = unsupported() {
        state.set(MicState::Blocked(why));
    }

    // The engine list is only advisory — dictation works without it, on whatever was last chosen —
    // so a failed fetch leaves the button alone rather than blocking it.
    spawn_local(async move {
        if let Ok(voice) = fetch::voice().await {
            let chosen = engine.get_untracked();
            // A remembered engine whose key has since been removed would fail on every press with
            // a message from the server. Falling back to the server's default keeps the button
            // working; the picker still shows what happened to the old choice.
            if !voice.engines.iter().any(|e| e.id == chosen && e.ready) {
                engine.set(voice.default_engine.clone());
                remember_engine(&voice.default_engine);
            }
            engines.set(voice.engines);
        }
    });

    let press = Callback::new(move |()| {
        if state.get_untracked() == MicState::Listening {
            match session.try_update_value(Option::take).flatten() {
                Some(live) => {
                    if let Some(settled) = live.stop() {
                        state.set(settled);
                    }
                }
                // No session behind a listening button. Nothing is running, so the button must
                // come back rather than stay lit over something that is not there.
                None => state.set(MicState::Idle),
            }
            return;
        }
        let engine_id = engine.get_untracked();
        let started = if engine_id == BROWSER {
            start_browser(value, state)
        } else {
            start_recording(value, state, engine_id)
        };
        match started {
            Ok(live) => {
                state.set(MicState::Listening);
                session.set_value(Some(live));
            }
            Err(why) => state.set(MicState::Blocked(why)),
        }
    });

    let hint = Signal::derive(move || {
        let id = engine.get();
        engines
            .get()
            .iter()
            .find(|e| e.id == id)
            .map_or(id, |e| format!("{} · {}", e.label, e.detail))
    });

    // Where the open menu should sit, in viewport coordinates. Measured from the button at the
    // moment it opens rather than expressed in CSS, for the reason spelled out on `picker`.
    let menu_style = RwSignal::new(String::new());
    let anchor = NodeRef::<html::Div>::new();

    view! {
        <div class="relative flex items-end" node_ref=anchor>
            <adi_ui::MicButton state=state on_press=press hint=hint/>
            {move || (!engines.get().is_empty())
                .then(|| picker(engines, engine, picking, state, anchor, menu_style))}
        </div>
    }
}

/// The engine chooser: a caret under the microphone, and the list it opens.
///
/// Beside the button rather than on a settings page, because the choice is one you revise *while*
/// dictating — the browser's recogniser mishears a name, and the fix is the next engine down, not
/// a trip across the app.
///
/// **The menu is `fixed` and positioned by measurement**, which looks like more work than
/// `absolute` and is not. The composer sits inside a panel that clips its overflow, and an
/// absolutely-positioned menu is clipped with it — the list came out one row tall, its other four
/// engines cut off at the panel's edge. A fixed element is not clipped by an ancestor's overflow,
/// but it also cannot be placed by CSS relative to that ancestor, so the button's rectangle is
/// read when the menu opens and the menu is pinned to the viewport instead.
fn picker(
    engines: RwSignal<Vec<VoiceEngineDto>>,
    engine: RwSignal<String>,
    picking: RwSignal<bool>,
    state: RwSignal<MicState>,
    anchor: NodeRef<html::Div>,
    menu_style: RwSignal<String>,
) -> impl IntoView {
    view! {
        <button
            class="absolute -right-1 -bottom-1 grid size-4 place-items-center rounded-full \
                   border border-line-strong bg-raise text-ink-3 hover:text-ink"
            type="button"
            title="Choose the speech engine"
            on:click=move |_| {
                picking.update(|open| *open = !*open);
                if picking.get_untracked() {
                    menu_style.set(anchored_above(anchor));
                }
            }
        >
            <adi_ui::Icon
                icon=adi_ui::Lucide::ChevronDown
                size=adi_ui::IconSize::Sm
                label="Choose the speech engine"
            />
        </button>
        {move || picking.get().then(|| view! {
            // An invisible sheet over the whole window, under the menu and above everything else.
            // It is what closes the menu when you click away — the alternative is a document
            // listener that has to be added, removed and taught not to fire on the click that
            // opened it. It also swallows the wheel, which keeps a viewport-pinned menu from
            // drifting away from the button it was measured against.
            <div class="fixed inset-0 z-40" on:click=move |_| picking.set(false)></div>
            <div class="fixed z-50 w-60 rounded-lg border border-line-strong bg-raise p-1"
                 style=move || menu_style.get()>
                <For
                    each=move || engines.get()
                    key=|e| e.id.clone()
                    let:option
                >
                    {
                        let id = option.id.clone();
                        let chosen = Signal::derive({
                            let id = id.clone();
                            move || engine.get() == id
                        });
                        view! {
                            <button
                                class="flex w-full flex-col items-start gap-0.5 rounded-md px-2 \
                                       py-1.5 text-left hover:bg-hover disabled:opacity-40 \
                                       disabled:hover:bg-transparent"
                                type="button"
                                // An engine with no key is shown but cannot be picked: hiding it
                                // would leave no hint that configuring it is even possible.
                                disabled=!option.ready
                                on:click={
                                    let id = id.clone();
                                    move |_| {
                                        engine.set(id.clone());
                                        remember_engine(&id);
                                        picking.set(false);
                                        // A blocked button may have been blocked by the *previous*
                                        // engine's failure, which the new one need not inherit.
                                        if matches!(state.get_untracked(), MicState::Blocked(_))
                                            && unsupported().is_none()
                                        {
                                            state.set(MicState::Idle);
                                        }
                                    }
                                }
                            >
                                // The chosen engine is said with a tick and a weight, never a
                                // colour — the composer's one orange is its send button.
                                <span class="flex items-center gap-1.5 text-row text-ink">
                                    <span class=move || if chosen.get() { "font-medium" } else { "" }>
                                        {option.label.clone()}
                                    </span>
                                    {chosen.get().then(|| view! {
                                        <adi_ui::Icon
                                            icon=adi_ui::Lucide::Check
                                            size=adi_ui::IconSize::Sm
                                            class="text-ink-2"
                                        />
                                    })}
                                </span>
                                <span class="text-mini text-ink-3">{option.detail.clone()}</span>
                            </button>
                        }
                    }
                </For>
            </div>
        })}
    }
}

/// CSS placing a fixed menu just above `anchor`, right edges aligned.
///
/// Stated as `right`/`bottom` rather than `left`/`top` so the menu's own size never enters into
/// it: growing the list moves its top edge, and the edge that matters — the one against the
/// button — stays put. `max(8)` keeps it off the window edge on a narrow screen.
fn anchored_above(anchor: NodeRef<html::Div>) -> String {
    let Some(element) = anchor.get_untracked() else {
        return String::new();
    };
    let rect = element.get_bounding_client_rect();
    let width = viewport("innerWidth");
    let height = viewport("innerHeight");
    format!(
        "right:{:.0}px;bottom:{:.0}px;",
        (width - rect.right()).max(8.0),
        (height - rect.top() + 6.0).max(8.0),
    )
}

/// One of the window's pixel dimensions.
fn viewport(name: &str) -> f64 {
    number(&window(), name).unwrap_or(0.0)
}

/// A dictation in progress, and how to end it.
enum Session {
    /// The browser's recogniser. Its event handlers are deliberately **not** held here — see
    /// `start_browser`, where they are leaked on purpose.
    Browser { recognition: JsValue },
    /// A recording bound for the server. Stopping fires `onstop`, which is what actually uploads.
    ///
    /// The recorder is behind an `Option` because the microphone permission prompt is answered
    /// *after* the session exists: the button must show Listening the moment it is pressed, and
    /// the recorder only appears once the user has said yes.
    Recording(Rc<RefCell<Option<MediaRecorder>>>),
}

impl Session {
    /// End it, answering with the state to show **at once**, or `None` to leave the button where
    /// it is because this session's own events will move it.
    fn stop(self) -> Option<MicState> {
        match self {
            Self::Browser { recognition } => {
                // `stop` finalises what has been heard; `abort` would throw it away. A user who
                // pressed the button expects the last sentence to survive the press.
                call_method(&recognition, "stop");
                // Idle now, rather than waiting for `onend`. The recogniser's own end event is the
                // wrong thing to hang the button on: it arrives a task later at best, and a
                // recogniser that has already stopped for its own reasons may never send it at
                // all — which leaves a button that says it is listening and cannot be pressed off.
                Some(MicState::Idle)
            }
            // Empty when stop beat the permission prompt. Nothing was ever recorded, so there is
            // nothing to stop and nothing to upload.
            //
            // `None`: stopping a recorder is the *start* of the work, not the end of it. `onstop`
            // moves the button to Working and the upload returns it to Idle.
            Self::Recording(recorder) => {
                if let Some(recorder) = recorder.borrow().as_ref() {
                    let _ = recorder.stop();
                    return None;
                }
                Some(MicState::Idle)
            }
        }
    }
}

/// Why dictation cannot run here at all, if it cannot.
///
/// The insecure-origin case is the one worth spelling out: `https://app.adi` is a secure context
/// and `http://app.adi` is not, they differ by four characters, and a browser denies the
/// microphone on the second without explaining itself.
fn unsupported() -> Option<String> {
    let window = window();
    let secure = js_sys::Reflect::get(&window, &JsValue::from_str("isSecureContext"))
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !secure {
        return Some(
            "Dictation needs a secure page — open the panel over https://app.adi".to_string(),
        );
    }
    None
}

/// Recognise speech in the page, writing words into `value` as they are said.
fn start_browser(value: RwSignal<String>, state: RwSignal<MicState>) -> Result<Session, String> {
    let ctor = ["SpeechRecognition", "webkitSpeechRecognition"]
        .iter()
        .find_map(|name| {
            js_sys::Reflect::get(&window(), &JsValue::from_str(name))
                .ok()
                .filter(JsValue::is_function)
        })
        .and_then(|found| found.dyn_into::<js_sys::Function>().ok())
        .ok_or_else(|| {
            "This browser has no speech recogniser — pick another engine, or use Chrome or Safari"
                .to_string()
        })?;

    let recognition = js_sys::Reflect::construct(&ctor, &js_sys::Array::new())
        .map_err(|_| "the speech recogniser would not start".to_string())?;

    set(&recognition, "continuous", &JsValue::TRUE);
    // Interim results are the whole reason to prefer this engine: without them nothing appears
    // until a pause, and the button looks broken for as long as the sentence takes.
    set(&recognition, "interimResults", &JsValue::TRUE);
    if let Some(lang) = document_language() {
        set(&recognition, "lang", &JsValue::from_str(&lang));
    }

    // What was in the box when dictation began. Kept whole: everything recognised is appended
    // after it, so speaking into a half-typed message extends it instead of eating it.
    let base = value.get_untracked();
    let committed = Rc::new(RefCell::new(String::new()));

    let on_result = {
        let committed = Rc::clone(&committed);
        let base = base.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
            let Ok(results) = js_sys::Reflect::get(&event, &JsValue::from_str("results")) else {
                return;
            };
            let length = count(&results, "length");
            // Everything before `resultIndex` the recogniser considers settled and will not send
            // again; re-reading from zero would duplicate every finalised phrase.
            let from = count(&event, "resultIndex");

            let mut interim = String::new();
            for index in from..length {
                let Ok(result) = js_sys::Reflect::get_u32(&results, index) else {
                    continue;
                };
                let Ok(alternative) = js_sys::Reflect::get_u32(&result, 0) else {
                    continue;
                };
                let Some(text) =
                    js_sys::Reflect::get(&alternative, &JsValue::from_str("transcript"))
                        .ok()
                        .and_then(|v| v.as_string())
                else {
                    continue;
                };
                let final_ = js_sys::Reflect::get(&result, &JsValue::from_str("isFinal"))
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if final_ {
                    committed.borrow_mut().push_str(&text);
                } else {
                    interim.push_str(&text);
                }
            }
            let heard = format!("{}{interim}", committed.borrow());
            value.set(join(&base, heard.trim()));
        })
    };

    let on_error = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let code = js_sys::Reflect::get(&event, &JsValue::from_str("error"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        state.set(match code.as_str() {
            // The one the user can act on, and the one a browser is most likely to raise.
            "not-allowed" | "service-not-allowed" => MicState::Blocked(
                "The microphone is blocked — allow it for this site in the browser's settings"
                    .to_string(),
            ),
            // Hearing nothing is not a failure worth latching the button for.
            "no-speech" | "aborted" => MicState::Idle,
            other => MicState::Blocked(format!("the recogniser stopped: {other}")),
        });
    });

    // `onend` fires however recognition ended — stopped, timed out, or failed — so it is the one
    // place that reliably returns the button to Idle.
    let on_end = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| {
        if state.get_untracked() == MicState::Listening {
            state.set(MicState::Idle);
        }
    });

    set(&recognition, "onresult", on_result.as_ref());
    set(&recognition, "onerror", on_error.as_ref());
    set(&recognition, "onend", on_end.as_ref());

    // Leaked on purpose, and this is the load-bearing line.
    //
    // Every one of these events arrives *after* the press that ends dictation: `stop()` is
    // asynchronous, and the recogniser still owes a final `onresult` and an `onend`. Holding the
    // closures in the `Session` meant they were dropped the moment the session was taken to be
    // stopped — and a dropped `Closure` throws when JavaScript calls it. So `onend` never ran, the
    // button never left Listening, and with the session already taken there was nothing left for a
    // second press to stop: dictation could be started and then never stopped.
    //
    // Three small closures per dictation is the price, and it is the right one to pay.
    on_result.forget();
    on_error.forget();
    on_end.forget();

    call_method(&recognition, "start");

    Ok(Session::Browser { recognition })
}

/// Record a clip and, on stop, send it to the server to be transcribed.
fn start_recording(
    value: RwSignal<String>,
    state: RwSignal<MicState>,
    engine: String,
) -> Result<Session, String> {
    let media_devices = window()
        .navigator()
        .media_devices()
        .map_err(|_| "this browser exposes no microphone".to_string())?;

    let constraints = MediaStreamConstraints::new();
    constraints.set_audio(&JsValue::TRUE);
    let request = media_devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|_| "the microphone could not be opened".to_string())?;

    let mime = CONTAINERS
        .iter()
        .find(|candidate| MediaRecorder::is_type_supported(candidate))
        .copied()
        .ok_or_else(|| "this browser records no audio format the server accepts".to_string())?;

    // Permission is a promise, and a `Session` is wanted now. So the button goes to Listening on
    // the caller's return and the recorder is attached when the user answers the prompt; pressing
    // stop before then finds no session and simply resets, which is the right outcome for a
    // recording that never began.
    let started: Rc<RefCell<Option<MediaRecorder>>> = Rc::new(RefCell::new(None));
    let handoff = Rc::clone(&started);
    let base = value.get_untracked();

    spawn_local(async move {
        let Ok(stream) = JsFuture::from(request).await else {
            state.set(MicState::Blocked(
                "The microphone is blocked — allow it for this site in the browser's settings"
                    .to_string(),
            ));
            return;
        };
        let stream = stream.unchecked_into::<MediaStream>();

        let options = MediaRecorderOptions::new();
        options.set_mime_type(mime);
        let Ok(recorder) =
            MediaRecorder::new_with_media_stream_and_media_recorder_options(&stream, &options)
        else {
            state.set(MicState::Blocked(
                "the recorder would not start".to_string(),
            ));
            return;
        };

        let chunks: Rc<RefCell<Vec<Blob>>> = Rc::new(RefCell::new(Vec::new()));
        let on_data = {
            let chunks = Rc::clone(&chunks);
            Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
                if let Some(blob) = event.dyn_ref::<BlobEvent>().and_then(BlobEvent::data)
                    && blob.size() > 0.0
                {
                    chunks.borrow_mut().push(blob);
                }
            })
        };

        let on_stop = {
            let chunks = Rc::clone(&chunks);
            let stream = stream.clone();
            Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| {
                // Release the microphone before the upload, not after: the browser's recording
                // indicator stays lit while any track is live, and leaving it on through a slow
                // transcription reads as still listening.
                for track in stream.get_tracks().iter() {
                    if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                        track.stop();
                    }
                }

                let parts = js_sys::Array::new();
                for blob in chunks.borrow().iter() {
                    parts.push(blob);
                }
                let bag = BlobPropertyBag::new();
                bag.set_type(mime);
                let Ok(clip) = Blob::new_with_blob_sequence_and_options(&parts, &bag) else {
                    state.set(MicState::Idle);
                    return;
                };
                if clip.size() <= 0.0 {
                    // Nothing was captured — a press and an immediate second press. Not an error.
                    state.set(MicState::Idle);
                    return;
                }

                state.set(MicState::Working);
                upload(clip, mime, engine.clone(), base.clone(), value, state);
            })
        };

        recorder.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));
        recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));
        // The handlers must outlive this task; the recorder holds the only reference that matters
        // and is dropped when the session is.
        on_data.forget();
        on_stop.forget();

        if recorder.start().is_err() {
            state.set(MicState::Blocked(
                "the recorder would not start".to_string(),
            ));
            return;
        }
        *handoff.borrow_mut() = Some(recorder);
    });

    Ok(Session::Recording(started))
}

/// Read the finished clip and hand it to the server, then write what came back into the composer.
///
/// `base` is the box's text from before dictation began rather than its text now, so a transcript
/// that lands late cannot append itself twice or swallow what was typed while waiting.
fn upload(
    clip: Blob,
    mime: &'static str,
    engine: String,
    base: String,
    value: RwSignal<String>,
    state: RwSignal<MicState>,
) {
    spawn_local(async move {
        let Ok(buffer) = JsFuture::from(clip.array_buffer()).await else {
            state.set(MicState::Blocked(
                "the recording could not be read".to_string(),
            ));
            return;
        };
        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
        match fetch::transcribe(&engine, mime, &bytes).await {
            Ok(transcript) => {
                let heard = transcript.text.trim();
                if !heard.is_empty() {
                    value.set(join(&base, heard));
                }
                state.set(MicState::Idle);
            }
            // The server's message names the provider and its complaint (a bad key, a rate
            // limit), which is what the user needs; it is shown rather than flattened to
            // "transcription failed".
            Err(why) => state.set(MicState::Blocked(why)),
        }
    });
}

/// Append `heard` to whatever was already typed, separated by a single space.
fn join(base: &str, heard: &str) -> String {
    let base = base.trim_end();
    if base.is_empty() {
        heard.to_string()
    } else if heard.is_empty() {
        base.to_string()
    } else {
        format!("{base} {heard}")
    }
}

/// The page's language, so the recogniser is not left guessing at an accent.
fn document_language() -> Option<String> {
    window()
        .navigator()
        .language()
        .filter(|lang| !lang.is_empty())
}

fn set(target: &JsValue, name: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(target, &JsValue::from_str(name), value);
}

fn number(target: &JsValue, name: &str) -> Option<f64> {
    js_sys::Reflect::get(target, &JsValue::from_str(name))
        .ok()
        .and_then(|v| v.as_f64())
}

/// A count or index property, which JavaScript keeps as an `f64` like every other number.
///
/// The guards are what make the conversion total rather than merely likely: a value that is not
/// finite, is negative, or is past `u32::MAX` is not an index into a list of recognised phrases,
/// and reading it as one would be worse than reading it as none.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the range checks immediately above make the cast exact"
)]
fn count(target: &JsValue, name: &str) -> u32 {
    let raw = number(target, name).unwrap_or(0.0);
    if !raw.is_finite() || raw <= 0.0 {
        return 0;
    }
    if raw >= f64::from(u32::MAX) {
        return u32::MAX;
    }
    raw as u32
}

/// Call a no-argument method by name, ignoring both a missing method and a throw. Both are
/// outcomes this module has nothing better to do about than carry on.
fn call_method(target: &JsValue, name: &str) {
    if let Ok(method) = js_sys::Reflect::get(target, &JsValue::from_str(name))
        && let Ok(method) = method.dyn_into::<js_sys::Function>()
    {
        let _ = method.call0(target);
    }
}

fn storage() -> Option<web_sys::Storage> {
    window().local_storage().ok().flatten()
}

fn stored_engine() -> Option<String> {
    storage()?
        .get_item(ENGINE_KEY)
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
}

fn remember_engine(id: &str) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(ENGINE_KEY, id);
    }
}

fn window() -> web_sys::Window {
    leptos::prelude::window()
}
