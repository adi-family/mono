// Emits `answer` with `{ id, replies }`; replies combine selected labels and free text.

import { AdiElement, define, esc } from "./base.ts";
import { renderMarkdown } from "./markdown.ts";

export interface AskOption {
  label: string;
  description?: string;
}

export interface AskQuestion {
  id?: string;
  header?: string;
  question: string;
  options?: AskOption[];
  multi_select?: boolean;
}

export interface Ask {
  id: string;
  asked_at?: number;
  headline?: string;
  note?: string;
  questions?: AskQuestion[];
  deadline?: number | null;
}

interface AnswerPick {
  chosen: Set<number>;
  typed: string;
}

function shortDuration(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

class AdiAsk extends AdiElement {
  static observedAttributes = ["busy"];

  #ask: Ask | null = null;
  #picks: AnswerPick[] = [];
  #clock = 0;

  /** The pending question — `null` draws nothing. A new id resets the answers; the same id keeps them. */
  get ask(): Ask | null {
    return this.#ask;
  }

  set ask(value: Ask | null) {
    const same = value && this.#ask && value.id === this.#ask.id;
    this.#ask = value;
    if (!same) this.#build();
  }

  override template(): string {
    return `
      <div class="head"><span>Waiting on you</span><span class="deadline"></span></div>
      <div class="md note"></div>
      <div class="questions"></div>
      <div class="foot"><button class="send" type="button" disabled>Answer</button>
        <span class="hint"></span></div>
    `;
  }

  override setup(): void {
    this.must<HTMLButtonElement>(".send").addEventListener("click", () => this.#send());
    this.#build();
  }

  override connectedCallback(): void {
    super.connectedCallback();
    this.#clock = window.setInterval(() => this.#tick(), 1000);
  }

  disconnectedCallback(): void {
    window.clearInterval(this.#clock);
  }

  override update(): void {
    const busy = this.hasAttribute("busy");
    for (const b of this.shadowRoot.querySelectorAll<HTMLButtonElement | HTMLInputElement>("button, input")) {
      if (b.classList.contains("send")) continue;
      b.disabled = busy;
    }
    this.#refresh();
  }

  #build(): void {
    const box = this.$(".questions");
    if (!box) return;
    const ask = this.#ask;
    this.hidden = !ask;
    if (!ask) return;
    const questions = ask.questions ?? [];
    this.#picks = questions.map(() => ({ chosen: new Set<number>(), typed: "" }));
    this.must(".note").replaceChildren(ask.note?.trim() ? renderMarkdown(ask.note) : "");
    const oneTap = questions.length === 1 && questions[0].options?.length && !questions[0].multi_select;
    this.must(".foot").hidden = Boolean(oneTap);
    box.replaceChildren(
      ...questions.map((q, index) => {
        const row = document.createElement("div");
        row.className = "q";
        row.innerHTML = `
          <div class="q-line">
            ${questions.length > 1 ? `<span class="num">${index + 1}.</span>` : ""}
            ${q.header ? `<span class="tag">${esc(q.header)}</span>` : ""}
            <span class="text">${esc(q.question)}</span>
            ${q.multi_select ? `<span class="any">(choose any)</span>` : ""}
          </div>
          ${q.options?.length ? `<div class="options"></div>` : ""}
          <input type="text" placeholder="${q.options?.length ? "Or say something else…" : "Your answer…"}">
        `;
        const pick = this.#picks[index];
        (q.options ?? []).forEach((option, o) => {
          const b = document.createElement("button");
          b.type = "button";
          b.className = "option";
          b.setAttribute("aria-pressed", "false");
          b.innerHTML = `${esc(option.label)}${option.description ? `<span class="desc">${esc(option.description)}</span>` : ""}`;
          b.addEventListener("click", () => {
            if (q.multi_select) {
              if (pick.chosen.has(o)) pick.chosen.delete(o);
              else pick.chosen.add(o);
            } else {
              pick.chosen = new Set(pick.chosen.has(o) ? [] : [o]);
            }
            for (const [i, other] of [...row.querySelectorAll(".option")].entries()) {
              other.setAttribute("aria-pressed", String(pick.chosen.has(i)));
            }
            if (oneTap && pick.chosen.size) this.#send();
            this.#refresh();
          });
          row.querySelector<HTMLElement>(".options")!.append(b);
        });
        const input = row.querySelector("input")!;
        input.addEventListener("input", () => {
          pick.typed = input.value;
          this.#refresh();
        });
        input.addEventListener("keydown", (ev) => {
          if (ev.key === "Enter" && !ev.isComposing) this.#send();
        });
        return row;
      }),
    );
    this.#tick();
    this.update();
  }

  #replies(): string[] {
    const questions = this.#ask?.questions ?? [];
    return questions.map((q, i) => {
      const pick = this.#picks[i] ?? { chosen: new Set<number>(), typed: "" };
      const labels = (q.options ?? []).filter((_, o) => pick.chosen.has(o)).map((o) => o.label);
      const typed = pick.typed.trim();
      if (labels.length && typed) return `${labels.join(", ")} — ${typed}`;
      return labels.length ? labels.join(", ") : typed;
    });
  }

  #refresh(): void {
    const send = this.$<HTMLButtonElement>(".send");
    if (!send) return;
    const answered = this.#replies().some((r) => r.trim());
    send.disabled = this.hasAttribute("busy") || !answered;
    this.must(".hint").textContent = answered
      ? "a question left blank is answered “(no answer)”"
      : "pick an option or write an answer";
  }

  #send(): void {
    const replies = this.#replies();
    if (this.hasAttribute("busy") || replies.every((r) => !r.trim())) return;
    this.emit("answer", { id: this.#ask?.id, replies });
  }

  #tick(): void {
    const at = this.#ask?.deadline;
    const line = this.$(".deadline");
    if (!line) return;
    if (!at) {
      line.textContent = "";
      return;
    }
    const left = at - Date.now();
    line.textContent = left <= 0 ? "taking its own default now" : `takes its own default in ${shortDuration(left)}`;
  }
}

define("adi-ask", AdiAsk);

export { AdiAsk };

declare global {
  interface HTMLElementTagNameMap {
    "adi-ask": AdiAsk;
  }
}
