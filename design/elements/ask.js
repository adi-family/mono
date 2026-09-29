// `<adi-ask>` — the block a run puts up when it needs a person to decide something: the
// JavaScript twin of `adi_ui::Ask` (crates/adi-ui/src/ask.rs), which says why it is a block and
// not a message.
//
//   ask.ask = { id, note, deadline, questions: [{ header, question, multi_select,
//                                                   options: [{ label, description }] }] };
//   ask.addEventListener("answer", (e) => post(e.detail.id, e.detail.replies));
//
// Drawn as §6 draws an ask: a 2px rule down the left and nothing else around it. One question
// with a fixed set of answers sends on the click; everything else has a Send, lit as soon as *any*
// question has an answer, so the one question nobody can settle never holds the rest hostage.
// Every question keeps a free-text box, because the right answer is regularly "neither — do this".
//
// A reply is the chosen labels, then anything typed, joined by an em dash: "Postgres — but read
// analytics off the replica" arrives as one sentence.

import { AdiElement, define, esc, sheet } from "./base.js";
import { MARKDOWN_SHEET, renderMarkdown } from "./markdown.js";

/** Milliseconds as the coarsest unit that still says something: `40s`, `12m`, `3h`, `2d`. */
function shortDuration(ms) {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

class AdiAsk extends AdiElement {
  static observedAttributes = ["busy"];

  static sheet = sheet(`
    :host { display: block; flex: none; max-width: 80ch; padding: 4px 0 4px 18px;
      border-left: 2px solid var(--line-strong); }
    .head { display: flex; align-items: center; gap: 12px; margin-bottom: 8px;
      font-size: var(--fs-label); color: var(--ink-3); }
    .deadline { margin-left: auto; }
    .md.note { margin-bottom: 12px; font-size: var(--fs-small); color: var(--ink-2); }
    .md.note:empty { display: none; }
    .questions { display: flex; flex-direction: column; gap: 16px; }
    .q { display: flex; flex-direction: column; gap: 8px; }
    .q-line { display: flex; flex-wrap: wrap; align-items: baseline; gap: 8px; }
    .num { font-size: var(--fs-label); color: var(--ink-3); }
    .tag { padding: 2px 8px; border-radius: var(--r-pill); background: var(--chip);
      font-size: var(--fs-label); color: var(--ink-2); }
    .text { font-size: 15px; color: var(--ink); }
    .any { font-size: var(--fs-label); color: var(--ink-3); }
    .options { display: flex; flex-wrap: wrap; gap: 8px; }
    .option { max-width: 100%; padding: 7px 14px; border-radius: var(--r); background: var(--btn);
      font-size: var(--fs-ui-sm); font-weight: 500; text-align: left; transition: background var(--transition); }
    .option:hover { background: var(--btn-hover); }
    .option[aria-pressed="true"] { background: var(--bg-active); }
    .option:disabled { opacity: .4; cursor: not-allowed; }
    .desc { display: block; margin-top: 2px; font-size: var(--fs-label); font-weight: 400; color: var(--ink-3); }
    input { width: 100%; padding: 8px 12px; border: 1px solid var(--line-strong); border-radius: var(--r);
      background: var(--bg-raise); color: var(--ink); font: inherit; font-size: var(--fs-ui); }
    input::placeholder { color: var(--ink-3); }
    .foot { display: flex; align-items: center; gap: 12px; margin-top: 12px; }
    .foot[hidden] { display: none; }
    .send { padding: 7px 14px; border-radius: var(--r); background: var(--btn);
      font-size: var(--fs-ui-sm); font-weight: 500; transition: background var(--transition); }
    .send:hover { background: var(--btn-hover); }
    .send:disabled { opacity: .4; cursor: not-allowed; }
    .hint { font-size: var(--fs-small); color: var(--ink-3); }
  `);

  #ask = null;
  #picks = [];
  #clock = 0;

  /** The pending question — `null` draws nothing. A new id resets the answers; the same id keeps them. */
  get ask() {
    return this.#ask;
  }

  set ask(value) {
    const same = value && this.#ask && value.id === this.#ask.id;
    this.#ask = value;
    if (!same) this.#build();
  }

  template() {
    return `
      <div class="head"><span>Waiting on you</span><span class="deadline"></span></div>
      <div class="md note"></div>
      <div class="questions"></div>
      <div class="foot"><button class="send" type="button" disabled>Answer</button>
        <span class="hint"></span></div>
    `;
  }

  setup() {
    this.shadowRoot.adoptedStyleSheets = [MARKDOWN_SHEET, ...this.shadowRoot.adoptedStyleSheets];
    this.$(".send").addEventListener("click", () => this.#send());
    this.#build();
  }

  connectedCallback() {
    super.connectedCallback();
    // The deadline is a clock the reader is watching, not decoration — one text node a second.
    this.#clock = window.setInterval(() => this.#tick(), 1000);
  }

  disconnectedCallback() {
    window.clearInterval(this.#clock);
  }

  update() {
    const busy = this.hasAttribute("busy");
    for (const b of this.shadowRoot.querySelectorAll("button, input")) {
      if (b.classList.contains("send")) continue;
      b.disabled = busy;
    }
    this.#refresh();
  }

  #build() {
    const box = this.$(".questions");
    if (!box) return;
    const ask = this.#ask;
    this.hidden = !ask;
    if (!ask) return;
    const questions = ask.questions ?? [];
    this.#picks = questions.map(() => ({ chosen: new Set(), typed: "" }));
    this.$(".note").replaceChildren(ask.note?.trim() ? renderMarkdown(ask.note) : "");
    const oneTap = questions.length === 1 && questions[0].options?.length && !questions[0].multi_select;
    this.$(".foot").hidden = Boolean(oneTap);
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
          row.querySelector(".options").append(b);
        });
        const input = row.querySelector("input");
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

  #replies() {
    const questions = this.#ask?.questions ?? [];
    return questions.map((q, i) => {
      const pick = this.#picks[i] ?? { chosen: new Set(), typed: "" };
      const labels = (q.options ?? []).filter((_, o) => pick.chosen.has(o)).map((o) => o.label);
      const typed = pick.typed.trim();
      if (labels.length && typed) return `${labels.join(", ")} — ${typed}`;
      return labels.length ? labels.join(", ") : typed;
    });
  }

  #refresh() {
    const send = this.$(".send");
    if (!send) return;
    const answered = this.#replies().some((r) => r.trim());
    send.disabled = this.hasAttribute("busy") || !answered;
    this.$(".hint").textContent = answered
      ? "a question left blank is answered “(no answer)”"
      : "pick an option or write an answer";
  }

  #send() {
    const replies = this.#replies();
    if (this.hasAttribute("busy") || replies.every((r) => !r.trim())) return;
    this.emit("answer", { id: this.#ask?.id, replies });
  }

  #tick() {
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
