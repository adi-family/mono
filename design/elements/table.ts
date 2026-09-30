// Set `columns` and `rows` as properties. Attributes: empty, sort, dir.

import { AdiElement, define } from "./base.js";
import "./icon.js";

export type TableRow = Record<string, unknown>;

export interface TableColumn {
  key: string;
  label?: string;
  align?: "left" | "center" | "right";
  width?: string;
  actions?: boolean;
  sortable?: boolean;
  mono?: boolean;
  muted?: boolean;
  compare?: (a: unknown, b: unknown, rowA: TableRow, rowB: TableRow) => number;
  format?: (value: unknown, row: TableRow) => unknown;
}

class AdiTable extends AdiElement {
  static observedAttributes = ["empty", "sort", "dir"];

  #columns: TableColumn[] = [];
  #rows: TableRow[] = [];

  override template(): string {
    return `
      <table part="table"><thead><tr part="head"></tr></thead><tbody part="body"></tbody></table>
      <div class="empty" part="empty" hidden></div>
    `;
  }

  /** The columns, left to right. */
  get columns(): TableColumn[] {
    return this.#columns;
  }

  set columns(columns: TableColumn[] | null | undefined) {
    this.#columns = columns ?? [];
    this.#draw();
  }

  /** The rows, as objects keyed by the columns' `key`. */
  get rows(): TableRow[] {
    return this.#rows;
  }

  set rows(rows: TableRow[] | null | undefined) {
    this.#rows = rows ?? [];
    this.#draw();
  }

  override update(): void {
    this.#draw();
  }

  get sorted(): TableRow[] {
    const key = this.attr("sort");
    const column = this.#columns.find((c) => c.key === key);
    if (!column) return this.#rows;
    const sign = this.attr("dir", "asc") === "desc" ? -1 : 1;
    const compare = column.compare ?? defaultCompare;
    return [...this.#rows].sort((a, b) => sign * compare(a[key], b[key], a, b));
  }

  #draw(): void {
    if (!this.shadowRoot.firstElementChild) return;
    const head = this.must<HTMLTableRowElement>("thead tr");
    const body = this.must<HTMLTableSectionElement>("tbody");
    const empty = this.must(".empty");
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
            if (event.target instanceof Element && event.target.closest("button, a, adi-button")) return;
            this.emit("select", { row, index });
          });
        }
        return tr;
      }),
    );

    const nothing = rows.length === 0;
    this.must<HTMLTableElement>("table").hidden = nothing;
    empty.hidden = !nothing;
    empty.textContent = this.attr("empty", "Nothing here yet");
  }

  #sortBy(key: string): void {
    const dir = this.attr("sort") === key && this.attr("dir", "asc") === "asc" ? "desc" : "asc";
    this.setAttribute("sort", key);
    this.setAttribute("dir", dir);
    this.emit("sort", { key, dir });
  }
}

function cell(td: HTMLTableCellElement, value: unknown): void {
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
function defaultCompare(a: unknown, b: unknown): number {
  const missing = (v: unknown) => v === null || v === undefined || v === "";
  if (missing(a) || missing(b)) return missing(a) && missing(b) ? 0 : missing(a) ? 1 : -1;
  if (typeof a === "number" && typeof b === "number") return a - b;
  return String(a).localeCompare(String(b), undefined, { numeric: true });
}

define("adi-table", AdiTable);

export { AdiTable };

declare global {
  interface HTMLElementTagNameMap {
    "adi-table": AdiTable;
  }
}
