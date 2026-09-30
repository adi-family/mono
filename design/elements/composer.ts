// Events: send/asap `{ text }`, stop, files `{ files }`, unattach `{ key }`.
// The caller uploads attachments and clears the message after a successful send.

import { AdiElement, define, esc } from "./base.js";
import "./icon.js";

const MAX_HEIGHT = 200;

export interface ComposerAttachment {
  key: string;
  name: string;
  preview?: string;
  image?: boolean;
  state: "uploading" | "ready" | "failed";
  error?: string;
}

class AdiComposer extends AdiElement {
  static observedAttributes = ["placeholder", "busy", "stoppable", "asap", "attach", "refusal"];

  #attachments: ComposerAttachment[] = [];

  override template(): string {
    return `
      <div class="box" part="box" title="Enter sends · Shift-Enter for a new line">
        <div class="tray" role="list"></div>
        <div class="row">
          <textarea rows="1" part="input"></textarea>
          <input type="file" multiple>
          <button class="btn clip" type="button" title="Attach a file — or paste one, or drop it here" hidden>
            <adi-icon name="paperclip" size="16" label="Attach a file"></adi-icon>
          </button>
          <slot name="tools"></slot>
          <button class="btn stop" type="button" title="Stop" hidden>
            <adi-icon name="square" size="16" label="Stop"></adi-icon>
          </button>
          <button class="btn asap" type="button" title="Send now — overtakes what is already waiting" hidden>
            <adi-icon name="zap" size="16" label="Send asap"></adi-icon>
          </button>
          <button class="btn send" type="button" title="Send" disabled>
            <adi-icon name="arrow-up" size="16" label="Send"></adi-icon>
          </button>
        </div>
        <div class="line"></div>
      </div>
    `;
  }

  override setup(): void {
    const area = this.must<HTMLTextAreaElement>("textarea");
    area.addEventListener("input", () => {
      this.#fit();
      this.update();
    });
    area.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" && !ev.shiftKey && !ev.altKey && !ev.ctrlKey && !ev.metaKey && !ev.isComposing) {
        ev.preventDefault();
        this.#send("send");
      }
    });
    area.addEventListener("paste", (ev) => {
      const files = [...(ev.clipboardData?.files ?? [])];
      if (files.length && this.hasAttribute("attach")) {
        ev.preventDefault();
        this.emit("files", { files });
      }
    });
    const box = this.must<HTMLDivElement>(".box");
    // Cancel dragover to prevent navigation to the dropped file.
    box.addEventListener("dragover", (ev) => {
      if (this.hasAttribute("attach")) ev.preventDefault();
    });
    box.addEventListener("drop", (ev) => {
      const files = [...(ev.dataTransfer?.files ?? [])];
      if (files.length && this.hasAttribute("attach")) {
        ev.preventDefault();
        this.emit("files", { files });
      }
    });
    const picker = this.must<HTMLInputElement>("input[type=file]");
    this.must<HTMLButtonElement>(".clip").addEventListener("click", () => picker.click());
    picker.addEventListener("change", () => {
      const files = [...(picker.files ?? [])];
      picker.value = "";
      if (files.length) this.emit("files", { files });
    });
    this.must<HTMLButtonElement>(".send").addEventListener("click", () => this.#send("send"));
    this.must<HTMLButtonElement>(".asap").addEventListener("click", () => this.#send("asap"));
    this.must<HTMLButtonElement>(".stop").addEventListener("click", () => this.emit("stop"));
  }

  get value(): string {
    return this.$<HTMLTextAreaElement>("textarea")?.value ?? "";
  }

  set value(text: string) {
    const area = this.$<HTMLTextAreaElement>("textarea");
    if (!area) return;
    area.value = text ?? "";
    this.#fit();
    this.update();
  }

  get attachments(): ComposerAttachment[] {
    return this.#attachments;
  }

  set attachments(list: ComposerAttachment[]) {
    this.#attachments = Array.isArray(list) ? list : [];
    this.#drawTray();
    this.update();
  }

  override focus(): void {
    this.$<HTMLTextAreaElement>("textarea")?.focus();
  }

  override update(): void {
    const area = this.must<HTMLTextAreaElement>("textarea");
    const busy = this.hasAttribute("busy");
    area.placeholder = this.attr("placeholder", "Write to the agent…");
    area.disabled = busy;
    const ready = this.#ready();
    this.must<HTMLButtonElement>(".send").disabled = !ready;
    this.must<HTMLButtonElement>(".asap").disabled = !ready;
    const stoppable = this.hasAttribute("stoppable");
    this.must<HTMLButtonElement>(".stop").hidden = !stoppable;
    this.must<HTMLButtonElement>(".asap").hidden = !(stoppable && this.hasAttribute("asap"));
    const attach = this.hasAttribute("attach");
    this.must<HTMLButtonElement>(".clip").hidden = !attach;
    const uploading = this.#attachments.some((a) => a.state === "uploading");
    this.must(".line").textContent = uploading ? "attaching…" : !attach ? this.attr("refusal") : "";
  }

  #ready(): boolean {
    const typed = Boolean(this.value.trim());
    const attached = this.#attachments.some((a) => a.state === "ready");
    const uploading = this.#attachments.some((a) => a.state === "uploading");
    return (typed || attached) && !uploading && !this.hasAttribute("busy");
  }

  #drawTray(): void {
    const tray = this.$(".tray");
    if (!tray) return;
    tray.replaceChildren(
      ...this.#attachments.map((a) => {
        const thumb = document.createElement("div");
        thumb.className = `thumb ${a.state}`;
        thumb.setAttribute("role", "listitem");
        thumb.title = a.error ? `${a.name} — ${a.error}` : a.name;
        thumb.innerHTML = `
          ${a.image && a.preview ? `<img alt="${esc(a.name)}" src="${esc(a.preview)}">` : `<div class="file"><adi-icon name="paperclip" size="14" label="File"></adi-icon><span>${esc(a.name)}</span></div>`}
          ${a.state === "uploading" ? `<div class="over">…</div>` : a.state === "failed" ? `<div class="over">failed</div>` : ""}
          <button class="remove" type="button" title="Remove this attachment">
            <adi-icon name="x" size="14" label="Remove ${esc(a.name)}"></adi-icon>
          </button>`;
        thumb.querySelector<HTMLButtonElement>(".remove")?.addEventListener("click", () => this.emit("unattach", { key: a.key }));
        return thumb;
      }),
    );
  }

  #send(kind: "send" | "asap"): void {
    if (!this.#ready()) return;
    this.emit(kind, { text: this.value });
  }

  // Reset before measuring so the textarea can shrink. Skip placeholder height when empty.
  #fit(): void {
    const area = this.must<HTMLTextAreaElement>("textarea");
    area.style.height = "auto";
    if (!area.value) return;
    area.style.height = `${Math.min(area.scrollHeight, MAX_HEIGHT)}px`;
  }
}

define("adi-composer", AdiComposer);

export { AdiComposer };

declare global {
  interface HTMLElementTagNameMap {
    "adi-composer": AdiComposer;
  }
}
