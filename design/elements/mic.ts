// Dictation appends to the composer. Browser recognition streams; other engines upload recorded audio.

import { AdiElement, define, esc } from "./base.ts";
import type { AdiComposer } from "./composer.ts";
import "./icon.ts";

const ENGINE_KEY = "adi.voice.engine.v1";
const BROWSER = "browser";
const CONTAINERS = ["audio/webm;codecs=opus", "audio/webm", "audio/mp4", "audio/ogg;codecs=opus"];
const BLOCKED = "The microphone is blocked — allow it for this site in the browser's settings";

type MicState = "idle" | "listening" | "working" | "blocked";

interface VoiceEngine {
  id: string;
  label: string;
  detail: string;
  ready: boolean;
}

interface VoiceResponse {
  engines?: VoiceEngine[];
  default_engine?: string;
}

interface TranscriptionResponse {
  text?: string;
  error?: string;
}

interface DictationSession {
  stop(): void;
}

interface RecognitionResult {
  readonly isFinal: boolean;
  readonly [index: number]: { readonly transcript: string };
}

interface RecognitionEvent extends Event {
  readonly resultIndex: number;
  readonly results: {
    readonly length: number;
    readonly [index: number]: RecognitionResult;
  };
}

interface RecognitionErrorEvent extends Event {
  readonly error: string;
}

interface BrowserRecognition {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  onresult: ((event: RecognitionEvent) => void) | null;
  onerror: ((event: RecognitionErrorEvent) => void) | null;
  onend: (() => void) | null;
  start(): void;
  stop(): void;
}

interface RecognitionWindow extends Window {
  SpeechRecognition?: new () => BrowserRecognition;
  webkitSpeechRecognition?: new () => BrowserRecognition;
}

function join(base: string, heard: string): string {
  if (!heard) return base;
  if (!base.trim()) return heard;
  return /\s$/.test(base) ? `${base}${heard}` : `${base} ${heard}`;
}

class AdiMic extends AdiElement {
  static observedAttributes = ["api"];

  #state: MicState = "idle";
  #why = "";
  #engines: VoiceEngine[] = [];
  #engine = localStorage.getItem(ENGINE_KEY) || BROWSER;
  #session: DictationSession | null = null;

  override template(): string {
    return `
      <button class="mic" type="button"></button>
      <button class="pick" type="button" title="Choose the speech engine" hidden>
        <adi-icon name="chevron-down" size="14" label="Choose the speech engine"></adi-icon>
      </button>
      <div class="menu" hidden></div>
    `;
  }

  override setup(): void {
    if (!window.isSecureContext) this.#block("Dictation needs a secure page — open the panel over https://app.adi");
    this.must<HTMLButtonElement>(".mic").addEventListener("click", () => this.#press());
    this.must<HTMLButtonElement>(".pick").addEventListener("click", (ev) => {
      ev.stopPropagation();
      this.#toggleMenu();
    });
    document.addEventListener("click", () => (this.must(".menu").hidden = true));
    this.#loadEngines();
  }

  override update(): void {
    const mic = this.$<HTMLButtonElement>(".mic");
    if (!mic) return;
    const engine = this.#engines.find((e) => e.id === this.#engine);
    const hint = engine ? `${engine.label} · ${engine.detail}` : this.#engine;
    const verbs: Partial<Record<MicState, string>> = { listening: "Stop dictating", working: "Working out what you said" };
    const verb = verbs[this.#state] ?? "Dictate a message";
    mic.title = this.#state === "blocked" ? this.#why : `${verb} · ${hint}`;
    mic.className = `mic ${this.#state}`;
    mic.disabled = this.#state === "working" || this.#state === "blocked";
    mic.setAttribute("aria-pressed", String(this.#state === "listening"));
    mic.innerHTML =
      this.#state === "working"
        ? `<adi-icon class="spin" name="loader-circle" size="16" label="Working out what you said"></adi-icon>`
        : `<adi-icon name="mic" size="16" label="${esc(verb)}"></adi-icon>`;
    this.must<HTMLButtonElement>(".pick").hidden = !this.#engines.length;
  }

  get #target(): AdiComposer | null {
    return this.closest<AdiComposer>("adi-composer");
  }

  #set(state: MicState): void {
    this.#state = state;
    this.update();
  }

  #block(why: string): void {
    this.#why = why;
    this.#set("blocked");
  }

  async #loadEngines(): Promise<void> {
    try {
      const res = await fetch(`${this.attr("api")}/api/voice`);
      if (!res.ok) return;
      const voice: VoiceResponse = await res.json();
      this.#engines = voice.engines ?? [];
      if (!this.#engines.some((e) => e.id === this.#engine && e.ready)) {
        this.#engine = voice.default_engine || BROWSER;
        localStorage.setItem(ENGINE_KEY, this.#engine);
      }
      this.update();
    } catch {
      // Fall back to browser recognition if the engine list is unavailable.
    }
  }

  #toggleMenu(): void {
    const menu = this.must<HTMLDivElement>(".menu");
    if (!menu.hidden) {
      menu.hidden = true;
      return;
    }
    // Place the menu above the button to avoid the composer’s overflow clipping.
    const r = this.getBoundingClientRect();
    menu.style.right = `${Math.max(8, window.innerWidth - r.right)}px`;
    menu.style.bottom = `${Math.max(8, window.innerHeight - r.top + 6)}px`;
    menu.replaceChildren(
      ...this.#engines.map((e) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = "option";
        b.disabled = !e.ready;
        const chosen = e.id === this.#engine;
        b.innerHTML = `<span class="name ${chosen ? "chosen" : ""}">${esc(e.label)}${chosen ? `<adi-icon name="check" size="14"></adi-icon>` : ""}</span>
          <span class="detail">${esc(e.detail)}</span>`;
        b.addEventListener("click", (ev) => {
          ev.stopPropagation();
          this.#engine = e.id;
          localStorage.setItem(ENGINE_KEY, e.id);
          menu.hidden = true;
          if (this.#state === "blocked" && window.isSecureContext) this.#set("idle");
          else this.update();
        });
        return b;
      }),
    );
    menu.hidden = false;
  }

  #press(): void {
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
      this.#block(err instanceof Error ? err.message : String(err));
    }
  }

  #startBrowser(): DictationSession {
    const browser: RecognitionWindow = window;
    const Recognition = browser.SpeechRecognition || browser.webkitSpeechRecognition;
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
      // Results before resultIndex are final and are not repeated.
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
    rec.onend = () => {
      if (this.#state === "listening") this.#set("idle");
    };
    rec.start();
    return {
      // stop preserves the final sentence; abort discards it.
      stop: () => {
        rec.stop();
        this.#set("idle");
      },
    };
  }

  #startRecording(): DictationSession {
    if (!navigator.mediaDevices?.getUserMedia) throw new Error("this browser exposes no microphone");
    const mime = CONTAINERS.find((c) => window.MediaRecorder?.isTypeSupported(c));
    if (!mime) throw new Error("this browser records no audio format the server accepts");
    const target = this.#target;
    const base = target?.value ?? "";
    const engine = this.#engine;
    let recorder: MediaRecorder | null = null;
    let stopped = false;
    navigator.mediaDevices.getUserMedia({ audio: true }).then(
      (stream) => {
        const chunks: Blob[] = [];
        recorder = new MediaRecorder(stream, { mimeType: mime });
        recorder.ondataavailable = (ev) => ev.data.size && chunks.push(ev.data);
        recorder.onstop = async () => {
          // Stop all tracks before uploading so the recording indicator turns off immediately.
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
            const data: TranscriptionResponse = await res.json().catch(() => ({}));
            if (!res.ok) throw new Error(data.error || `${res.status}`);
            if (target) target.value = join(base, (data.text ?? "").trim());
            this.#set("idle");
          } catch (err) {
            this.#block(`transcription failed: ${err instanceof Error ? err.message : String(err)}`);
          }
        };
        recorder.start();
        // Recording was cancelled while the permission prompt was pending.
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

declare global {
  interface HTMLElementTagNameMap {
    "adi-mic": AdiMic;
  }
}
