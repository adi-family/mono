// Set `columns` and `rows` as properties. Attributes: empty, sort, dir.

import { AdiElement, define } from "./base.ts";
import "./icon.ts";

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
    const column = this.#columns.find((column) => column.key === key);
    if (!column) return this.#rows;

    const direction = this.attr("dir", "asc") === "desc" ? -1 : 1;
    const compare = column.compare ?? defaultCompare;
    return [...this.#rows].sort((left, right) => direction * compare(left[key], right[key], left, right));
  }

  #draw(): void {
    if (!this.shadowRoot.firstElementChild) return;
    const head = this.must<HTMLTableRowElement>("thead tr");
    const body = this.must<HTMLTableSectionElement>("tbody");
    const empty = this.must(".empty");
    const sortKey = this.attr("sort");
    const sortDirection = this.attr("dir", "asc");

    head.replaceChildren(
      ...this.#columns.map((column) => this.#buildHeader(column, sortKey, sortDirection)),
    );

    const rows = this.sorted;
    body.replaceChildren(...rows.map((row, index) => this.#buildRow(row, index)));

    const hasRows = rows.length > 0;
    this.must<HTMLTableElement>("table").hidden = !hasRows;
    empty.hidden = hasRows;
    empty.textContent = this.attr("empty", "Nothing here yet");
  }

  #buildHeader(column: TableColumn, sortKey: string, sortDirection: string): HTMLTableCellElement {
    const header = document.createElement("th");
    if (column.align) header.setAttribute("align", column.align);
    if (column.width) header.style.width = column.width;
    if (column.actions) header.classList.add("actions");

    const sortable = column.sortable !== false && !column.actions;
    if (!sortable) {
      header.textContent = column.label ?? "";
      return header;
    }

    const isSorted = column.key === sortKey;
    const descending = sortDirection === "desc";
    header.classList.add("sortable");
    if (isSorted) header.setAttribute("aria-sort", descending ? "descending" : "ascending");

    const button = document.createElement("button");
    button.type = "button";
    button.append(column.label ?? "");

    const arrow = document.createElement("adi-icon");
    arrow.setAttribute("name", isSorted && descending ? "arrow-down" : "arrow-up");
    arrow.setAttribute("size", "14");
    button.append(arrow);
    button.addEventListener("click", () => this.#sortBy(column.key));
    header.append(button);

    return header;
  }

  #buildRow(row: TableRow, index: number): HTMLTableRowElement {
    const rowElement = document.createElement("tr");
    for (const column of this.#columns) {
      const cell = document.createElement("td");
      if (column.mono) cell.classList.add("mono");
      if (column.muted) cell.classList.add("muted");
      if (column.align) cell.setAttribute("align", column.align);
      if (column.actions) cell.classList.add("actions");

      const value = row[column.key];
      const content = column.format ? column.format(value, row) : value;
      renderCellContent(cell, content);
      rowElement.append(cell);
    }

    if (this.hasAttribute("selectable")) {
      rowElement.addEventListener("click", (event) => {
        const clickedControl = event.target instanceof Element
          && event.target.closest("button, a, adi-button");
        if (clickedControl) return;
        this.emit("select", { row, index });
      });
    }

    return rowElement;
  }

  #sortBy(key: string): void {
    const alreadyAscending = this.attr("sort") === key && this.attr("dir", "asc") === "asc";
    const dir = alreadyAscending ? "desc" : "asc";
    this.setAttribute("sort", key);
    this.setAttribute("dir", dir);
    this.emit("sort", { key, dir });
  }
}

function renderCellContent(cell: HTMLTableCellElement, value: unknown): void {
  if (value instanceof Node) {
    cell.append(value);
    return;
  }
  if (value === null || value === undefined || value === "") {
    const placeholder = document.createElement("span");
    placeholder.className = "nothing";
    placeholder.textContent = "—";
    cell.append(placeholder);
    return;
  }
  cell.textContent = String(value);
}

/** Numbers numerically, everything else as text; an empty cell sorts last going up. */
function defaultCompare(left: unknown, right: unknown): number {
  const leftMissing = left === null || left === undefined || left === "";
  const rightMissing = right === null || right === undefined || right === "";
  if (leftMissing && rightMissing) return 0;
  if (leftMissing) return 1;
  if (rightMissing) return -1;

  if (typeof left === "number" && typeof right === "number") return left - right;
  return String(left).localeCompare(String(right), undefined, { numeric: true });
}

define("adi-table", AdiTable);

export { AdiTable };

declare global {
  interface HTMLElementTagNameMap {
    "adi-table": AdiTable;
  }
}
