// `<adi-table>` — §6 "Table", which is as much a list of prohibitions as a specification: no
// card around it, header 12px `--ink-3` sentence case, rows separated by hairlines, identifier
// columns in mono one step dimmer, repeated values dimmed, `—` for nothing, and the row's
// actions behind a ⋯ at the far right rather than a column of the word "Open".
//
//   const table = document.querySelector("adi-table");
//   table.columns = [
//     { key: "name", label: "Service" },
//     { key: "port", label: "Port", mono: true, align: "right" },
//     { key: "state", label: "State" },        // a value may be a DOM node — a status, a pill
//   ];
//   table.rows = [{ name: "dev-ui", port: 9080, state: statusEl }];
//
// Columns and rows are properties, not attributes: a table is fed from data, and JSON in an
// attribute is a string that has to be parsed, escaped and kept in step with itself. What *is*
// an attribute is everything a person would write by hand — `empty`, `sort`, `dir`.
//
// Sorting is in here rather than at the call site because the header is the control for it:
// clicking one and having nothing happen unless the page wired it up is the kind of half-built
// component this library exists to stop.

import { AdiElement, define, sheet } from "./base.js";
import "./icon.js";

class AdiTable extends AdiElement {
  static observedAttributes = ["empty", "sort", "dir"];

  static sheet = sheet(`
    :host { display: block; overflow-x: auto; }

    table { width: 100%; border-collapse: collapse; font-size: var(--fs-ui-sm); }
    th, td { text-align: left; padding: 9px 10px; white-space: nowrap; vertical-align: top; }
    th:first-child, td:first-child { padding-left: 0; }
    th:last-child, td:last-child { padding-right: 0; }

    th {
      padding-top: 8px;
      padding-bottom: 8px;
      font-size: var(--fs-label);
      font-weight: 500;
      color: var(--ink-3);
      border-bottom: 1px solid var(--line-strong);
    }
    /* The whole header cell is the sort target, so the button fills it. */
    th.sortable { padding: 0; }
    th.sortable > button {
      display: flex;
      align-items: center;
      gap: 4px;
      width: 100%;
      padding: 8px 10px;
      font: inherit;
      color: inherit;
      text-align: inherit;
    }
    th.sortable:first-child > button { padding-left: 0; }
    th.sortable[align="right"] > button { justify-content: flex-end; }
    th.sortable > button adi-icon { opacity: 0; transition: opacity var(--transition); }
    th.sortable > button:hover { color: var(--ink-2); }
    th.sortable > button:hover adi-icon { opacity: .5; }
    th[aria-sort] > button { color: var(--ink-2); }
    th[aria-sort] > button adi-icon { opacity: 1; }

    td { border-bottom: 1px solid var(--line); color: var(--ink); }
    tbody tr:hover td { background: var(--bg-hover); }
    :host([selectable]) tbody tr { cursor: pointer; }

    td.mono { font-family: var(--mono); font-size: var(--fs-mono); color: var(--ink-2); }
    td.muted, .nothing { color: var(--ink-3); }
    td[align="right"] { text-align: right; font-variant-numeric: tabular-nums; }
    /* Row controls: shrink-wrapped against the right edge. */
    td.actions, th.actions { text-align: right; width: 1%; }

    .empty { padding: 24px 0; color: var(--ink-3); font-size: var(--fs-small); }
    .empty[hidden] { display: none; }
    table[hidden] { display: none; }
  `);

  #columns = [];
  #rows = [];

  template() {
    return `
      <table part="table"><thead><tr part="head"></tr></thead><tbody part="body"></tbody></table>
      <div class="empty" part="empty" hidden></div>
    `;
  }

  /** The columns, left to right. */
  get columns() {
    return this.#columns;
  }

  set columns(columns) {
    this.#columns = columns ?? [];
    this.#draw();
  }

  /** The rows, as objects keyed by the columns' `key`. */
  get rows() {
    return this.#rows;
  }

  set rows(rows) {
    this.#rows = rows ?? [];
    this.#draw();
  }

  update() {
    this.#draw();
  }

  /** The rows in the order they are shown — sorted, if a sort is set. */
  get sorted() {
    const key = this.attr("sort");
    const column = this.#columns.find((c) => c.key === key);
    if (!column) return this.#rows;
    const sign = this.attr("dir", "asc") === "desc" ? -1 : 1;
    const compare = column.compare ?? defaultCompare;
    return [...this.#rows].sort((a, b) => sign * compare(a[key], b[key], a, b));
  }

  #draw() {
    if (!this.shadowRoot.firstElementChild) return;
    const head = this.$("thead tr");
    const body = this.$("tbody");
    const empty = this.$(".empty");
    const sort = this.attr("sort");
    const dir = this.attr("dir", "asc");

    head.replaceChildren(
      ...this.#columns.map((column) => {
        const th = document.createElement("th");
        if (column.align) th.setAttribute("align", column.align);
        if (column.width) th.style.width = column.width;
        if (column.actions) th.classList.add("actions");
        const sortable = column.sortable !== false && !column.actions;
        if (!sortable) {
          th.textContent = column.label ?? "";
          return th;
        }
        th.classList.add("sortable");
        if (column.key === sort) th.setAttribute("aria-sort", dir === "desc" ? "descending" : "ascending");
        const button = document.createElement("button");
        button.type = "button";
        button.append(column.label ?? "");
        const arrow = document.createElement("adi-icon");
        arrow.setAttribute("name", column.key === sort && dir === "desc" ? "arrow-down" : "arrow-up");
        arrow.setAttribute("size", "14");
        button.append(arrow);
        button.addEventListener("click", () => this.#sortBy(column.key));
        th.append(button);
        return th;
      }),
    );

    const rows = this.sorted;
    body.replaceChildren(
      ...rows.map((row, index) => {
        const tr = document.createElement("tr");
        for (const column of this.#columns) {
          const td = document.createElement("td");
          if (column.mono) td.classList.add("mono");
          if (column.muted) td.classList.add("muted");
          if (column.align) td.setAttribute("align", column.align);
          if (column.actions) td.classList.add("actions");
          cell(td, column.format ? column.format(row[column.key], row) : row[column.key]);
          tr.append(td);
        }
        if (this.hasAttribute("selectable")) {
          tr.addEventListener("click", (event) => {
            // A ⋯ menu or a link in the row is its own action, not a selection of the row.
            if (event.target.closest("button, a, adi-button")) return;
            this.emit("select", { row, index });
          });
        }
        return tr;
      }),
    );

    const nothing = rows.length === 0;
    this.$("table").hidden = nothing;
    empty.hidden = !nothing;
    empty.textContent = this.attr("empty", "Nothing here yet");
  }

  #sortBy(key) {
    // Same column: turn it around. A different one: start ascending, which is what somebody
    // clicking an unsorted column means by it.
    const dir = this.attr("sort") === key && this.attr("dir", "asc") === "asc" ? "desc" : "asc";
    this.setAttribute("sort", key);
    this.setAttribute("dir", dir);
    this.emit("sort", { key, dir });
  }
}

/** Put one value in a cell: a node as it stands, nothing as `—`, anything else as text. */
function cell(td, value) {
  if (value instanceof Node) {
    td.append(value);
    return;
  }
  if (value === null || value === undefined || value === "") {
    const nothing = document.createElement("span");
    nothing.className = "nothing";
    nothing.textContent = "—";
    td.append(nothing);
    return;
  }
  td.textContent = String(value);
}

/** Numbers numerically, everything else as text; an empty cell sorts last going up. */
function defaultCompare(a, b) {
  const missing = (v) => v === null || v === undefined || v === "";
  if (missing(a) || missing(b)) return missing(a) && missing(b) ? 0 : missing(a) ? 1 : -1;
  if (typeof a === "number" && typeof b === "number") return a - b;
  return String(a).localeCompare(String(b), undefined, { numeric: true });
}

define("adi-table", AdiTable);

export { AdiTable };
