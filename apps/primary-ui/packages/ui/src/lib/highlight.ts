/*
 * A small syntax highlighter — everything CodeEditor knows about language.
 *
 * Hand-rolled on purpose: a grammar library would outweigh the rest of the component library,
 * and what gets edited here is a handful of formats. Each language is a scanner, good enough to
 * make structure visible — not a parser, and never one that can fail on odd input.
 *
 * Every scanner is total: an unterminated string or comment runs to the end of input and is
 * still emitted, so no text can throw or lose characters. Concatenating the runs always
 * reproduces the source exactly, which is what keeps the painted layer under the caret.
 */

export type CodeLanguage = "sh" | "ts" | "json" | "plain";

/** What a run of characters is, which is all the renderer needs to pick a colour. */
export type CodeToken =
  | "plain"
  | "comment"
  | "str"
  | "num"
  /** An assignment target or an object key — the left-hand side. */
  | "key"
  /** A language keyword or literal. */
  | "kw"
  /** A called function, a property being reached, or a shell `$expansion`. */
  | "func"
  | "punct";

export type CodeRun = readonly [token: CodeToken, text: string];

export const codeLanguageLabels: Record<CodeLanguage, string> = { sh: "Shell", ts: "TypeScript", json: "JSON", plain: "Plain text" };

const TS_KEYWORDS = new Set([
  "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete", "do", "else", "enum", "export",
  "extends", "false", "finally", "for", "from", "function", "if", "implements", "import", "in", "instanceof", "interface", "let", "new",
  "null", "of", "return", "static", "super", "switch", "this", "throw", "true", "try", "type", "typeof", "undefined", "var", "void",
  "while", "yield",
]);
const SH_KEYWORDS = new Set([
  "case", "do", "done", "elif", "else", "esac", "exit", "export", "fi", "for", "function", "if", "in", "local", "read", "return", "select",
  "shift", "then", "until", "while",
]);

const isDigit = (c: string) => c >= "0" && c <= "9";
const isAlpha = (c: string) => (c >= "a" && c <= "z") || (c >= "A" && c <= "Z");
const isWord = (c: string) => isAlpha(c) || isDigit(c) || c === "_";

/** The index just past a run of characters satisfying `test`, starting at `i`. */
function past(source: string, i: number, test: (c: string) => boolean): number {
  while (i < source.length && test(source[i])) i += 1;
  return i;
}

/** The index just past the quoted string opening at `i`, honouring backslash escapes. */
function pastString(source: string, i: number): number {
  const quote = source[i];
  i += 1;
  while (i < source.length) {
    const c = source[i];
    i += 1;
    if (c === "\\" && i < source.length) i += 1;
    else if (c === quote) break;
  }
  return i;
}

type Emit = (token: CodeToken, from: number, to: number) => void;

function scanJson(source: string, emit: Emit) {
  let i = 0;
  while (i < source.length) {
    const c = source[i];
    if (c === '"') {
      const next = pastString(source, i);
      // A string is a key when the next non-space character is a colon.
      const after = past(source, next, (ch) => ch === " " || ch === "\t" || ch === "\n" || ch === "\r");
      emit(source[after] === ":" ? "key" : "str", i, next);
      i = next;
    } else if (isDigit(c) || c === "-") {
      const next = past(source, i + 1, (ch) => isWord(ch) || ch === "." || ch === "-" || ch === "+");
      emit("num", i, next);
      i = next;
    } else if (isAlpha(c)) {
      const next = past(source, i, isAlpha);
      const word = source.slice(i, next);
      emit(word === "true" || word === "false" || word === "null" ? "kw" : "plain", i, next);
      i = next;
    } else {
      emit("{}[],:".includes(c) ? "punct" : "plain", i, i + 1);
      i += 1;
    }
  }
}

function scanTs(source: string, emit: Emit) {
  let i = 0;
  while (i < source.length) {
    const c = source[i];
    if (c === "/" && source[i + 1] === "/") {
      const next = past(source, i, (ch) => ch !== "\n");
      emit("comment", i, next);
      i = next;
    } else if (c === "/" && source[i + 1] === "*") {
      // A block comment runs to its closing `*/`, or to the end of input if never closed.
      const close = source.indexOf("*/", i + 2);
      const next = close < 0 ? source.length : close + 2;
      emit("comment", i, next);
      i = next;
    } else if (c === '"' || c === "'" || c === "`") {
      const next = pastString(source, i);
      emit("str", i, next);
      i = next;
    } else if (isDigit(c)) {
      const next = past(source, i, (ch) => isWord(ch) || ch === ".");
      emit("num", i, next);
      i = next;
    } else if (isAlpha(c) || c === "_" || c === "$") {
      const next = past(source, i, (ch) => isWord(ch) || ch === "$");
      let token: CodeToken = "plain";
      if (TS_KEYWORDS.has(source.slice(i, next))) token = "kw";
      else {
        // A name is a `func` when it is called or reached through a dot — that covers both
        // `join(...)` and `process.env`, which is what a module is skimmed for.
        const after = past(source, next, (ch) => ch === " " || ch === "\t");
        if (source[after] === "(" || (i > 0 && source[i - 1] === ".")) token = "func";
      }
      emit(token, i, next);
      i = next;
    } else {
      emit("{}[]();,.:".includes(c) ? "punct" : "plain", i, i + 1);
      i += 1;
    }
  }
}

/** Whether `i` opens a word: nothing but whitespace or an operator precedes it. */
function atWordStart(source: string, i: number): boolean {
  return i === 0 || " \t\n;|&()".includes(source[i - 1]);
}

/** The index past a `$` expansion: `$NAME`, `${NAME:-default}` or `$(command)`. */
function pastExpansion(source: string, i: number): number {
  const open = source[i + 1];
  if (open !== "{" && open !== "(") return past(source, i + 1, isWord);
  const close = open === "{" ? "}" : ")";
  let depth = 0;
  let j = i + 1;
  while (j < source.length) {
    if (source[j] === open) depth += 1;
    else if (source[j] === close) {
      depth -= 1;
      if (depth === 0) return j + 1;
    }
    j += 1;
  }
  return j;
}

/**
 * Shell. Beyond comments and strings, the thing worth getting right is `$VARIABLE` expansion:
 * a code block's settings arrive that way, so they should stand out.
 */
function scanSh(source: string, emit: Emit) {
  let i = 0;
  while (i < source.length) {
    const c = source[i];
    if (c === "#" && atWordStart(source, i)) {
      // A `#` opens a comment only at the start of a word: `${X#prefix}` and `a#b` are not comments.
      const next = past(source, i, (ch) => ch !== "\n");
      emit("comment", i, next);
      i = next;
    } else if (c === '"' || c === "'") {
      const next = pastString(source, i);
      emit("str", i, next);
      i = next;
    } else if (c === "$") {
      const next = pastExpansion(source, i);
      emit("func", i, next);
      i = next;
    } else if (isDigit(c) && (atWordStart(source, i) || source[i - 1] === "=")) {
      // A number opens a word or is assigned (`retries=3`); the digits in `file2` and `v1.2` are part of a name.
      const next = past(source, i, (ch) => isWord(ch) || ch === ".");
      emit("num", i, next);
      i = next;
    } else if (isAlpha(c) || c === "_") {
      const next = past(source, i, (ch) => isWord(ch) || ch === "-");
      // `name=value` — the assignment target reads like a key.
      emit(SH_KEYWORDS.has(source.slice(i, next)) ? "kw" : source[next] === "=" ? "key" : "plain", i, next);
      i = next;
    } else {
      emit("|&;()<>{}".includes(c) ? "punct" : "plain", i, i + 1);
      i += 1;
    }
  }
}

const scanners: Record<Exclude<CodeLanguage, "plain">, (source: string, emit: Emit) => void> = { sh: scanSh, ts: scanTs, json: scanJson };

/** Beyond this the text is painted plain: highlighting is an enhancement, never a gate. */
export const highlightLimit = 400_000;

/**
 * Split `source` into consecutive `[token, text]` runs. Neighbouring runs of one token are
 * merged, so the DOM gets a handful of spans per line instead of one per character.
 */
export function highlightCode(language: CodeLanguage, source: string): CodeRun[] {
  if (!source) return [];
  const scan = language === "plain" || source.length > highlightLimit ? undefined : scanners[language];
  if (!scan) return [["plain", source]];
  const runs: [CodeToken, number, number][] = [];
  scan(source, (token, from, to) => {
    if (to <= from) return;
    const last = runs[runs.length - 1];
    if (last && last[0] === token) last[2] = to;
    else runs.push([token, from, to]);
  });
  return runs.map(([token, from, to]) => [token, source.slice(from, to)]);
}
