// Events: send/asap `{ text }`, stop, files `{ files }`, unattach `{ key }`.
// The caller uploads attachments and clears the message after a successful send.

import { AdiElement, define, esc } from "./base.ts";
import "./icon.ts";

const MAX_HEIGHT = 200;

// A paste with both text and files is a spreadsheet app's picture of the cells it also put on
// the clipboard as text — see the `paste` listener below — not a screenshot, which has no text.
function wantsFiles(text: string, fileCount: number): boolean {
  return fileCount > 0 && text.trim() === "";
}

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
    this.#setupInput();
    this.#setupAttachments();
    this.must<HTMLButtonElement>(".send").addEventListener("click", () => this.#send("send"));
    this.must<HTMLButtonElement>(".asap").addEventListener("click", () => this.#send("asap"));
    this.must<HTMLButtonElement>(".stop").addEventListener("click", () => this.emit("stop"));
  }

  #setupInput(): void {
    const textarea = this.must<HTMLTextAreaElement>("textarea");
    textarea.addEventListener("input", () => {
      this.#fit();
      this.update();
    });
    textarea.addEventListener("keydown", (event) => {
      const hasModifier = event.shiftKey || event.altKey || event.ctrlKey || event.metaKey;
      if (event.key !== "Enter" || hasModifier || event.isComposing) return;

      event.preventDefault();
      this.#send("send");
    });
  }

  #setupAttachments(): void {
    this.must<HTMLTextAreaElement>("textarea").addEventListener("paste", (event) => {
      const text = event.clipboardData?.getData("text/plain") ?? "";
      const files = event.clipboardData?.files;
      this.#receiveFiles(event, wantsFiles(text, files?.length ?? 0) ? files : undefined);
    });

    const box = this.must<HTMLDivElement>(".box");
    // Cancel dragover to prevent navigation to the dropped file.
    box.addEventListener("dragover", (event) => {
      if (this.hasAttribute("attach")) event.preventDefault();
    });
    box.addEventListener("drop", (event) => {
      this.#receiveFiles(event, event.dataTransfer?.files);
    });

    const picker = this.must<HTMLInputElement>("input[type=file]");
    this.must<HTMLButtonElement>(".clip").addEventListener("click", () => picker.click());
    picker.addEventListener("change", () => {
      const files = [...(picker.files ?? [])];
      picker.value = "";
      if (files.length) this.emit("files", { files });
    });
  }

  #receiveFiles(event: ClipboardEvent | DragEvent, fileList: FileList | undefined): void {
    const files = [...(fileList ?? [])];
    if (!files.length || !this.hasAttribute("attach")) return;

    event.preventDefault();
    this.emit("files", { files });
  }

  get value(): string {
    return this.$<HTMLTextAreaElement>("textarea")?.value ?? "";
  }

  set value(text: string) {
    const textarea = this.$<HTMLTextAreaElement>("textarea");
    if (!textarea) return;
    textarea.value = text ?? "";
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
    const textarea = this.must<HTMLTextAreaElement>("textarea");
    const busy = this.hasAttribute("busy");
    textarea.placeholder = this.attr("placeholder", "Write to the agent…");
    textarea.disabled = busy;

    const ready = this.#ready();
    this.must<HTMLButtonElement>(".send").disabled = !ready;
    this.must<HTMLButtonElement>(".asap").disabled = !ready;

    const stoppable = this.hasAttribute("stoppable");
    this.must<HTMLButtonElement>(".stop").hidden = !stoppable;
    this.must<HTMLButtonElement>(".asap").hidden = !(stoppable && this.hasAttribute("asap"));

    const canAttach = this.hasAttribute("attach");
    this.must<HTMLButtonElement>(".clip").hidden = !canAttach;

    const uploading = this.#attachments.some((attachment) => attachment.state === "uploading");
    let attachmentStatus = "";
    if (uploading) attachmentStatus = "attaching…";
    else if (!canAttach) attachmentStatus = this.attr("refusal");
    this.must(".line").textContent = attachmentStatus;
  }

  #ready(): boolean {
    const hasText = Boolean(this.value.trim());
    const hasAttachment = this.#attachments.some((attachment) => attachment.state === "ready");
    const uploading = this.#attachments.some((attachment) => attachment.state === "uploading");
    return (hasText || hasAttachment) && !uploading && !this.hasAttribute("busy");
  }

  #drawTray(): void {
    const tray = this.$(".tray");
    if (!tray) return;
    tray.replaceChildren(...this.#attachments.map((attachment) => this.#buildAttachment(attachment)));
  }

  #buildAttachment(attachment: ComposerAttachment): HTMLDivElement {
    const thumbnail = document.createElement("div");
    thumbnail.className = `thumb ${attachment.state}`;
    thumbnail.setAttribute("role", "listitem");
    thumbnail.title = attachment.error ? `${attachment.name} — ${attachment.error}` : attachment.name;

    const name = esc(attachment.name);
    let preview = `<div class="file"><adi-icon name="paperclip" size="14" label="File"></adi-icon><span>${name}</span></div>`;
    if (attachment.image && attachment.preview) {
      preview = `<img alt="${name}" src="${esc(attachment.preview)}">`;
    }

    let overlay = "";
    if (attachment.state === "uploading") overlay = `<div class="over">…</div>`;
    else if (attachment.state === "failed") overlay = `<div class="over">failed</div>`;

    thumbnail.innerHTML = `
      ${preview}
      ${overlay}
      <button class="remove" type="button" title="Remove this attachment">
        <adi-icon name="x" size="14" label="Remove ${name}"></adi-icon>
      </button>`;
    thumbnail.querySelector<HTMLButtonElement>(".remove")?.addEventListener("click", () => {
      this.emit("unattach", { key: attachment.key });
    });
    return thumbnail;
  }

  #send(kind: "send" | "asap"): void {
    if (!this.#ready()) return;
    this.emit(kind, { text: this.value });
  }

  // Reset before measuring so the textarea can shrink. Skip placeholder height when empty.
  #fit(): void {
    const textarea = this.must<HTMLTextAreaElement>("textarea");
    textarea.style.height = "auto";
    if (!textarea.value) return;
    textarea.style.height = `${Math.min(textarea.scrollHeight, MAX_HEIGHT)}px`;
  }
}

define("adi-composer", AdiComposer);

export { AdiComposer };

declare global {
  interface HTMLElementTagNameMap {
    "adi-composer": AdiComposer;
  }
}
