// `<adi-mic>` — dictate into the composer it sits in: the JavaScript twin of the panel's
// `voice::mic` (crates/adi-webapp/src/voice.rs) and `adi_ui::MicButton`.
//
//   <adi-composer><adi-mic slot="tools" api=""></adi-mic></adi-composer>
//
// Two kinds of engine, picked from the chevron at its corner — the list is the server's
// (`GET /api/voice`), and the choice is kept under the panel's own key, so a person who picked one
// there has picked it here:
//
// - **browser**: the page's own `SpeechRecognition`, with interim results, so words appear while
//   they are spoken rather than after a pause.
// - **anything else**: the clip is recorded (`MediaRecorder`), and on the second press sent to
//   `POST /api/voice/transcribe?engine=<id>` as raw bytes with the recorder's own type.
//
// Either way what is heard is appended after whatever was in the box when dictation began, so
// speaking into a half-typed message extends it rather than eating it.
//
// States, on the button: idle · listening (red-tinted: it is recording you) · working (the clip is
// being transcribed — a spinner the press started) · blocked (the reason is its title).

import { AdiElement, define, esc, sheet } from "./base.js";
import "./icon.js";

/** The panel's key for the chosen engine — shared, so the choice follows the person. */
const ENGINE_KEY = "adi.voice.engine.v1";
const BROWSER = "browser";
/** What a recorder may produce, best first — the containers the server's engines accept. */
const CONTAINERS = ["audio/webm;codecs=opus", "audio/webm", "audio/mp4", "audio/ogg;codecs=opus"];
const BLOCKED = "The microphone is blocked — allow it for this site in the browser's settings";

function join(base, heard) {
  if (!heard) return base;
  if (!base.trim()) return heard;
  return /\s$/.test(base) ? `${base}${heard}` : `${base} ${heard}`;
}

class AdiMic extends AdiElement {
  static observedAttributes = ["api"];

  static sheet = sheet(`
    :host { position: relative; display: flex; align-items: flex-end; }
    .mic { display: grid; place-items: center; width: 32px; height: 32px; border-radius: var(--r);
      color: var(--ink-2); transition: background var(--transition), color var(--transition); }
    .mic:hover { background: var(--bg-hover); color: var(--ink); }
    .mic.listening { background: var(--err-soft); color: var(--err); }
    .mic.working { color: var(--ink-3); }
    .mic:disabled { opacity: .4; cursor: not-allowed; }
    .spin { animation: spin 1s linear infinite; }
    @keyframes spin { to { transform: rotate(360deg); } }
    .pick { position: absolute; right: -4px; bottom: -4px; display: grid; place-items: center;
      width: 16px; height: 16px; border: 1px solid var(--line-strong); border-radius: var(--r-pill);
      background: var(--bg-raise); color: var(--ink-3); }
    .pick:hover { color: var(--ink); }
    .pick[hidden] { display: none; }
    .menu { position: fixed; z-index: 50; width: 240px; padding: 4px; border: 1px solid var(--line-strong);
      border-radius: var(--r-lg); background: var(--bg-raise); }
    .menu[hidden] { display: none; }
    .option { display: flex; flex-direction: column; align-items: flex-start; gap: 2px; width: 100%;
      padding: 6px 8px; border-radius: var(--r); text-align: left; }
    .option:hover { background: var(--bg-hover); }
    .option:disabled { opacity: .4; cursor: not-allowed; }
    .option:disabled:hover { background: none; }
    .name { display: flex; align-items: center; gap: 6px; font-size: var(--fs-ui-sm); color: var(--ink); }
    .name.chosen { font-weight: 500; }
    .detail { font-size: var(--fs-label); color: var(--ink-3); }
  `);

  #state = "idle";
  #why = "";
  #engines = [];
  #engine = localStorage.getItem(ENGINE_KEY) || BROWSER;
  #session = null;

  template() {
    return `
      <button class="mic" type="button"></button>
      <button class="pick" type="button" title="Choose the speech engine" hidden>
        <adi-icon name="chevron-down" size="14" label="Choose the speech engine"></adi-icon>
      </button>
      <div class="menu" hidden></div>
    `;
  }

  setup() {
    if (!window.isSecureContext) this.#block("Dictation needs a secure page — open the panel over https://app.adi");
    this.$(".mic").addEventListener("click", () => this.#press());
    this.$(".pick").addEventListener("click", (ev) => {
      ev.stopPropagation();
      this.#toggleMenu();
    });
    document.addEventListener("click", () => (this.$(".menu").hidden = true));
    this.#loadEngines();
  }

  update() {
    const mic = this.$(".mic");
    if (!mic) return;
    const engine = this.#engines.find((e) => e.id === this.#engine);
    const hint = engine ? `${engine.label} · ${engine.detail}` : this.#engine;
    const verb = { listening: "Stop dictating", working: "Working out what you said" }[this.#state] ?? "Dictate a message";
    mic.title = this.#state === "blocked" ? this.#why : `${verb} · ${hint}`;
    mic.className = `mic ${this.#state}`;
    mic.disabled = this.#state === "working" || this.#state === "blocked";
    mic.setAttribute("aria-pressed", String(this.#state === "listening"));
    mic.innerHTML =
      this.#state === "working"
        ? `<adi-icon class="spin" name="loader-circle" size="16" label="Working out what you said"></adi-icon>`
        : `<adi-icon name="mic" size="16" label="${esc(verb)}"></adi-icon>`;
    this.$(".pick").hidden = !this.#engines.length;
  }

  /** The composer this dictates into — the one it is slotted in. */
  get #target() {
    return this.closest("adi-composer");
  }

  #set(state) {
    this.#state = state;
    this.update();
  }

  #block(why) {
    this.#why = why;
    this.#set("blocked");
  }

  async #loadEngines() {
    try {
      const res = await fetch(`${this.attr("api")}/api/voice`);
      if (!res.ok) return;
      const voice = await res.json();
      this.#engines = voice.engines ?? [];
      if (!this.#engines.some((e) => e.id === this.#engine && e.ready)) {
        this.#engine = voice.default_engine || BROWSER;
        localStorage.setItem(ENGINE_KEY, this.#engine);
      }
      this.update();
    } catch {
      // No engine list: the browser's own recogniser is still there to try.
    }
  }

  #toggleMenu() {
    const menu = this.$(".menu");
    if (!menu.hidden) {
      menu.hidden = true;
      return;
    }
    // Fixed and placed above the button: the composer clips, and a menu cut in half by the box it
    // opened from is a menu with half its engines missing.
    const r = this.getBoundingClientRect();
    menu.style.right = `${Math.max(8, window.innerWidth - r.right)}px`;
    menu.style.bottom = `${Math.max(8, window.innerHeight - r.top + 6)}px`;
    menu.replaceChildren(
      ...this.#engines.map((e) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = "option";
        // An engine with no key is shown but cannot be picked: hiding it would leave no hint that
        // configuring it is even possible.
        b.disabled = !e.ready;
        const chosen = e.id === this.#engine;
        b.innerHTML = `<span class="name ${chosen ? "chosen" : ""}">${esc(e.label)}${chosen ? `<adi-icon name="check" size="14"></adi-icon>` : ""}</span>
          <span class="detail">${esc(e.detail)}</span>`;
        b.addEventListener("click", (ev) => {
          ev.stopPropagation();
          this.#engine = e.id;
          localStorage.setItem(ENGINE_KEY, e.id);
          menu.hidden = true;
          // A block may have been the previous engine's, which the new one need not inherit.
          if (this.#state === "blocked" && window.isSecureContext) this.#set("idle");
          else this.update();
        });
        return b;
      }),
    );
    menu.hidden = false;
  }

  #press() {
    if (this.#state === "listening") {
      const session = this.#session;
      this.#session = null;
      session?.stop();
      return;
    }
    if (this.#state !== "idle") return;
    try {
      this.#session = this.#engine === BROWSER ? this.#startBrowser() : this.#startRecording();
      this.#set("listening");
    } catch (err) {
      this.#block(err.message);
    }
  }

  #startBrowser() {
    const Recognition = window.SpeechRecognition || window.webkitSpeechRecognition;
    if (!Recognition) throw new Error("This browser has no speech recogniser — pick another engine, or use Chrome or Safari");
    const rec = new Recognition();
    rec.continuous = true;
    rec.interimResults = true;
    if (document.documentElement.lang) rec.lang = document.documentElement.lang;
    const target = this.#target;
    const base = target?.value ?? "";
    let committed = "";
    rec.onresult = (ev) => {
      let interim = "";
      // Everything before `resultIndex` is settled and will not be sent again.
      for (let i = ev.resultIndex; i < ev.results.length; i += 1) {
        const text = ev.results[i][0].transcript;
        if (ev.results[i].isFinal) committed += text;
        else interim += text;
      }
      if (target) target.value = join(base, `${committed}${interim}`.trim());
    };
    rec.onerror = (ev) => {
      if (ev.error === "not-allowed" || ev.error === "service-not-allowed") this.#block(BLOCKED);
      else if (ev.error === "no-speech" || ev.error === "aborted") this.#set("idle");
      else this.#block(`the recogniser stopped: ${ev.error}`);
    };
    // However recognition ended — stopped, timed out, failed — this is where the button comes back.
    rec.onend = () => {
      if (this.#state === "listening") this.#set("idle");
    };
    rec.start();
    return {
      // `stop` keeps the last sentence; `abort` would throw it away.
      stop: () => {
        rec.stop();
        this.#set("idle");
      },
    };
  }

  #startRecording() {
    if (!navigator.mediaDevices?.getUserMedia) throw new Error("this browser exposes no microphone");
    const mime = CONTAINERS.find((c) => window.MediaRecorder?.isTypeSupported(c));
    if (!mime) throw new Error("this browser records no audio format the server accepts");
    const target = this.#target;
    const base = target?.value ?? "";
    const engine = this.#engine;
    let recorder = null;
    let stopped = false;
    navigator.mediaDevices.getUserMedia({ audio: true }).then(
      (stream) => {
        const chunks = [];
        recorder = new MediaRecorder(stream, { mimeType: mime });
        recorder.ondataavailable = (ev) => ev.data.size && chunks.push(ev.data);
        recorder.onstop = async () => {
          // The microphone goes before the upload: the browser's recording light stays on while any
          // track is live, and a lit light through a slow transcription reads as still listening.
          stream.getTracks().forEach((t) => t.stop());
          const clip = new Blob(chunks, { type: mime });
          if (!clip.size) return this.#set("idle");
          this.#set("working");
          try {
            const res = await fetch(`${this.attr("api")}/api/voice/transcribe?engine=${encodeURIComponent(engine)}`, {
              method: "POST",
              headers: { "content-type": mime },
              body: clip,
            });
            const data = await res.json().catch(() => ({}));
            if (!res.ok) throw new Error(data.error || `${res.status}`);
            if (target) target.value = join(base, (data.text ?? "").trim());
            this.#set("idle");
          } catch (err) {
            this.#block(`transcription failed: ${err.message}`);
          }
        };
        recorder.start();
        // A stop pressed before permission was granted: nothing was ever recorded.
        if (stopped) recorder.stop();
      },
      () => this.#block(BLOCKED),
    );
    return {
      stop: () => {
        stopped = true;
        if (recorder && recorder.state !== "inactive") recorder.stop();
        else if (!recorder) this.#set("idle");
      },
    };
  }
}

define("adi-mic", AdiMic);

export { AdiMic };
