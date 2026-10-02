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
    const container = this.$(".questions");
    if (!container) return;

    const ask = this.#ask;
    this.hidden = !ask;
    if (!ask) return;

    const questions = ask.questions ?? [];
    this.#picks = questions.map(() => ({ chosen: new Set<number>(), typed: "" }));
    this.must(".note").replaceChildren(ask.note?.trim() ? renderMarkdown(ask.note) : "");

    const submitOnSelection = questions.length === 1
      && (questions[0].options?.length ?? 0) > 0
      && !questions[0].multi_select;
    this.must(".foot").hidden = submitOnSelection;

    container.replaceChildren(
      ...questions.map((question, index) => this.#buildQuestion(question, index, submitOnSelection)),
    );
    this.#tick();
    this.update();
  }

  #buildQuestion(question: AskQuestion, index: number, submitOnSelection: boolean): HTMLDivElement {
    const answer = this.#picks[index];
    const options = question.options ?? [];
    const showNumber = this.#picks.length > 1;
    const placeholder = options.length ? "Or say something else…" : "Your answer…";

    const row = document.createElement("div");
    row.className = "q";
    row.innerHTML = `
      <div class="q-line">
        ${showNumber ? `<span class="num">${index + 1}.</span>` : ""}
        ${question.header ? `<span class="tag">${esc(question.header)}</span>` : ""}
        <span class="text">${esc(question.question)}</span>
        ${question.multi_select ? `<span class="any">(choose any)</span>` : ""}
      </div>
    `;

    if (options.length) {
      row.append(this.#buildOptions(question, answer, submitOnSelection));
    }

    const input = document.createElement("input");
    input.type = "text";
    input.placeholder = placeholder;
    input.addEventListener("input", () => {
      answer.typed = input.value;
      this.#refresh();
    });
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.isComposing) this.#send();
    });
    row.append(input);

    return row;
  }

  #buildOptions(question: AskQuestion, answer: AnswerPick, submitOnSelection: boolean): HTMLDivElement {
    const container = document.createElement("div");
    container.className = "options";

    for (const [optionIndex, option] of (question.options ?? []).entries()) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "option";
      button.setAttribute("aria-pressed", "false");
      const description = option.description ? `<span class="desc">${esc(option.description)}</span>` : "";
      button.innerHTML = `${esc(option.label)}${description}`;

      button.addEventListener("click", () => {
        const wasSelected = answer.chosen.has(optionIndex);
        if (!question.multi_select) answer.chosen.clear();

        if (wasSelected) answer.chosen.delete(optionIndex);
        else answer.chosen.add(optionIndex);

        container.querySelectorAll(".option").forEach((optionButton, index) => {
          optionButton.setAttribute("aria-pressed", String(answer.chosen.has(index)));
        });

        if (submitOnSelection && answer.chosen.size > 0) this.#send();
        this.#refresh();
      });
      container.append(button);
    }

    return container;
  }

  #replies(): string[] {
    const questions = this.#ask?.questions ?? [];

    return questions.map((question, index) => {
      const answer = this.#picks[index];
      if (!answer) return "";

      const selectedLabels = (question.options ?? [])
        .filter((_, optionIndex) => answer.chosen.has(optionIndex))
        .map((option) => option.label);
      const selectedAnswer = selectedLabels.join(", ");
      const typedAnswer = answer.typed.trim();

      if (selectedLabels.length === 0) return typedAnswer;
      if (!typedAnswer) return selectedAnswer;
      return `${selectedAnswer} — ${typedAnswer}`;
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
