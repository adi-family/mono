# Voice mode — dictating to an agent

Speak a message instead of typing it. The microphone button sits in the agent composers at
`app.adi` → **Agents**: press it, talk, press it again, and the words land **in the box as an
editable draft** — never sent. Recognition is wrong often enough that a transcript posted
straight to an agent would be a sentence nobody wrote.

It is dictation, not a conversation: replies are not spoken back.

## Choosing an engine

The caret on the microphone opens the engine list. The choice is remembered per browser, in
`localStorage` under `adi.voice.engine.v1` — which recogniser is worth using depends on the
machine you are sitting at, so the server does not hold this preference.

| Engine | Model | Audio leaves the machine | Key |
|---|---|---|---|
| **Browser** | the browser's own recogniser | no | — |
| OpenAI | `gpt-4o-transcribe` | yes | `OPENAI_API_KEY` |
| Groq | `whisper-large-v3-turbo` | yes | `GROQ_API_KEY` |
| ElevenLabs | `scribe_v1` | yes | `ELEVENLABS_API_KEY` |
| Deepgram | `nova-3` | yes | `DEEPGRAM_API_KEY` |

**Browser** works with no setup and shows words *as they are said*, because the recogniser streams
interim guesses. It needs Chrome or Safari; Firefox has no `SpeechRecognition`. Accuracy is the
worst of the five.

The rest record the clip and upload it. Nothing appears until transcription finishes, but the
words are markedly better — and the API key stays on the server, never in the page.

An engine with no key is **listed but greyed**, saying which secret to set. Hiding it would leave
no hint that configuring it was possible. Set the key either as a secret:

```bash
printf %s "sk-…" | adi-mono secrets set OPENAI_API_KEY   # value from stdin, not shell history
```

…or as an environment variable in the app's environment. The store is checked first, the
environment second — so a key already exported for the agent loop works here without being
entered twice.

## Use `https://app.adi`, not `http://`

**A browser gives no microphone to an insecure page.** Loopback earns that exemption only under
the literal name `localhost`, never a hostname that merely resolves there — so `http://app.adi` is
refused and `https://app.adi` works. The front door already terminates TLS for exactly this class
of reason (see `adi-hive/src/tls.rs`); trust `ca.pem` once and the microphone works.

The button says so rather than failing silently: on an insecure page it is visibly out, and its
tooltip names the URL to use instead.

## How it is put together

```
adi-ui/src/voice.rs          MicButton — four states (Idle/Listening/Working/Blocked). Presentational:
                             it owns no microphone and knows no service.
adi-ui/src/composer.rs       Composer gained a `mic` slot. A slot, not a `voice` flag, so the
                             component library never reaches for a microphone or the network.

adi-webapp/src/voice.rs      The capture. Browser route drives SpeechRecognition; remote route
                             records with MediaRecorder and uploads. Engine choice + persistence.
adi-webapp/src/fetch.rs      `voice()` and `transcribe()`.
adi-webapp/…/agents/actions.rs   Mounts the mic in both composers (reply bar, launch bar).

adi-webapp-api/src/types.rs      VoiceState / VoiceEngineDto / Transcript.
adi-webapp-api/…/handlers/voice.rs   GET /api/voice, POST /api/voice/transcribe. Holds the keys and
                             settles the four providers' disagreements in one place.
adi-app/src/main.rs          Routes both endpoints.
```

### Things worth not rediscovering

- **`SpeechRecognition` is not in `web-sys`** without the `web_sys_unstable_apis` cfg, so
  `adi-webapp`'s `voice` reaches it through `js_sys::Reflect`. Reflection is needed anyway: the
  constructor is `webkitSpeechRecognition` on Chrome and Safari and `SpeechRecognition` in the
  spec, so it must be looked up by name at run time regardless.
- **The clip is posted as a raw body**, not JSON. It is already bytes with a `Content-Type` from
  `MediaRecorder`; base64 in a JSON field would add a third for nothing.
- **Containers differ by browser.** Chrome and Firefox record WebM/Opus, Safari records MP4 and
  nothing else. `MediaRecorder::is_type_supported` picks; handing Safari a type it does not support
  yields a silently empty clip.
- **The providers disagree about everything.** Three take `multipart/form-data`; Deepgram takes the
  bytes raw with the model in the query string. ElevenLabs names the model field `model_id` rather
  than `model` — and ignores `model` silently, failing as if none was given. All four bury the text
  at a different depth of the response JSON.
- **The engine menu is `fixed` and positioned by measurement.** The composer sits inside a panel
  that clips its overflow; an absolutely-positioned menu was clipped to one row, its other four
  engines cut off. A backdrop closes it on a click away and swallows the wheel, so a
  viewport-pinned menu cannot drift from the button it was measured against.
- **A stored engine whose key was since removed** falls back to the server's default rather than
  failing on every press.
