/*
 * The edits a code editor makes on the writer's behalf — indent, outdent, a new line that keeps
 * its indentation — as pure functions over the text and the selection, so they can be checked
 * without a DOM. Each answers one replacement plus where the selection should land.
 */

export type CodeEdit = {
  /** Replace `[from, to)` with `insert`. */
  from: number;
  to: number;
  insert: string;
  /** The selection once the replacement is in. */
  selectionStart: number;
  selectionEnd: number;
};

/** The start of the line holding `offset`. */
function lineStart(value: string, offset: number): number {
  return value.lastIndexOf("\n", offset - 1) + 1;
}

/** The end of the block of whole lines a selection touches: a selection ending at a line's start does not take that line. */
function blockEnd(value: string, start: number, end: number): number {
  const last = end > start && value[end - 1] === "\n" ? end - 1 : end;
  const newline = value.indexOf("\n", last);
  return newline < 0 ? value.length : newline;
}

/**
 * Tab. With a caret, insert one indent at it. With a selection, indent every line it touches
 * (blank lines stay blank) and keep those lines selected.
 */
export function indentEdit(value: string, start: number, end: number, unit = "  "): CodeEdit {
  if (start === end) return { from: start, to: end, insert: unit, selectionStart: start + unit.length, selectionEnd: start + unit.length };
  const from = lineStart(value, start);
  const to = blockEnd(value, start, end);
  const lines = value.slice(from, to).split("\n");
  const insert = lines.map((line) => (line ? unit + line : line)).join("\n");
  const first = lines[0] ? unit.length : 0;
  return { from, to, insert, selectionStart: start + first, selectionEnd: end + (insert.length - (to - from)) };
}

/** How much of a line's leading whitespace one outdent removes: a tab, or up to one indent of spaces. */
function outdentWidth(line: string, unit: string): number {
  if (line.startsWith("\t")) return 1;
  let width = 0;
  while (width < unit.length && line[width] === " ") width += 1;
  return width;
}

/** Shift+Tab. Remove one indent from every line the selection touches; `null` when there is none to remove. */
export function outdentEdit(value: string, start: number, end: number, unit = "  "): CodeEdit | null {
  const from = lineStart(value, start);
  const to = blockEnd(value, start, end);
  const lines = value.slice(from, to).split("\n");
  const widths = lines.map((line) => outdentWidth(line, unit));
  const removed = widths.reduce((sum, width) => sum + width, 0);
  if (!removed) return null;
  const insert = lines.map((line, index) => line.slice(widths[index])).join("\n");
  // The caret never moves onto the previous line: it stops at its own line's start.
  const selectionStart = Math.max(from, start - widths[0]);
  return { from, to, insert, selectionStart, selectionEnd: Math.max(selectionStart, end - removed) };
}

/** Enter. The new line starts at the indentation of the one the caret is leaving. */
export function newlineEdit(value: string, start: number, end: number): CodeEdit {
  const from = lineStart(value, start);
  const indent = /^[ \t]*/.exec(value.slice(from, start))?.[0] ?? "";
  const insert = `\n${indent}`;
  return { from: start, to: end, insert, selectionStart: start + insert.length, selectionEnd: start + insert.length };
}

/** A caret offset as the 1-based line and column a status line shows. */
export function caretPosition(value: string, offset: number): { line: number; column: number } {
  let line = 1;
  let last = -1;
  for (let i = value.indexOf("\n"); i >= 0 && i < offset; i = value.indexOf("\n", i + 1)) {
    line += 1;
    last = i;
  }
  return { line, column: offset - last };
}

/** How many lines `value` has; an empty text is one line. */
export function lineCount(value: string): number {
  let count = 1;
  for (let i = value.indexOf("\n"); i >= 0; i = value.indexOf("\n", i + 1)) count += 1;
  return count;
}
