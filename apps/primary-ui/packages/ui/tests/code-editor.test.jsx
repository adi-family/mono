import { describe, expect, test } from "bun:test";
import * as React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { CodeEditor, caretPosition, highlightCode, highlightLimit, indentEdit, lineCount, newlineEdit, outdentEdit } from "../src/components";

const text = (runs) => runs.map(([, run]) => run).join("");
const tokens = (runs) => runs.filter(([token]) => token !== "plain").map(([token, run]) => `${token}:${run}`);
/** Apply an edit the way the editor does, and mark the resulting selection with `[` and `]`. */
const applied = (value, edit) => {
  const next = value.slice(0, edit.from) + edit.insert + value.slice(edit.to);
  return `${next.slice(0, edit.selectionStart)}[${next.slice(edit.selectionStart, edit.selectionEnd)}]${next.slice(edit.selectionEnd)}`;
};

describe("highlightCode", () => {
  const samples = {
    sh: '#!/bin/sh\n# poll forever\nwhile true; do\n  url="${ADI_URL:-http://x}"\n  curl -fsS "$url" | head -c 3500 > "$ADI_PAYLOAD_FILE"\n  count=$((count + 1))\n  sleep 21600 # six hours\ndone\n',
    ts: '// watch\nconst every = Number(process.env.ADI_INTERVAL_SECS ?? 300) * 1000;\n/* block\n comment */\nfor (;;) {\n  const body = await (await fetch(url)).text();\n  console.log(`changed: ${body.length}`, \'x\', true);\n}\n',
    json: '{ "name": "deploy", "retries": 3, "ok": true, "tags": ["a", null], "ratio": -0.5 }\n',
  };

  test("the runs always reproduce the source exactly, for every language and for odd input", () => {
    for (const [language, source] of Object.entries(samples)) {
      for (const sample of [source, source.slice(0, 37), source.slice(11), `${source}"unterminated`, `${source}/* never closed`, `${source}\${never closed`, "🙂 $X # ünïcödé", "\t\r\n"]) {
        expect(text(highlightCode(language, sample))).toBe(sample);
        expect(text(highlightCode("plain", sample))).toBe(sample);
      }
    }
    expect(highlightCode("sh", "")).toEqual([]);
  });

  test("shell: comments, strings, expansions, keywords, assignments and operators", () => {
    expect(tokens(highlightCode("sh", 'count=1\nwhile true; do\n  echo "$HOME" ${X#pre} $(date) a#b # note\ndone'))).toEqual([
      "key:count", "num:1", "kw:while", "punct:;", "kw:do", 'str:"$HOME"', "func:${X#pre}", "func:$(date)", "comment:# note", "kw:done",
    ]);
    // A `#` inside a word is not a comment, and a digit inside a word is not a number.
    expect(tokens(highlightCode("sh", "tar -x file2 v1.2"))).toEqual([]);
    expect(tokens(highlightCode("sh", "sleep 21600"))).toEqual(["num:21600"]);
  });

  test("typescript: comments, strings, numbers, keywords, calls and members", () => {
    expect(tokens(highlightCode("ts", "const n = await fetch(url); // go\nprocess.env.X ?? `t${n}`"))).toEqual([
      "kw:const", "kw:await", "func:fetch", "punct:(", "punct:);", "comment:// go", "punct:.", "func:env", "punct:.", "func:X", "str:`t${n}`",
    ]);
    expect(tokens(highlightCode("ts", "/* a\n b */ 42"))).toEqual(["comment:/* a\n b */", "num:42"]);
  });

  test("json: keys are told from string values", () => {
    expect(tokens(highlightCode("json", '{"a": "b", "n": -1.5, "t": true}'))).toEqual([
      "punct:{", 'key:"a"', "punct::", 'str:"b"', "punct:,", 'key:"n"', "punct::", "num:-1.5", "punct:,", 'key:"t"', "punct::", "kw:true", "punct:}",
    ]);
  });

  test("neighbouring runs of one token are merged, and oversized text is painted plain", () => {
    expect(highlightCode("sh", "a b c")).toEqual([["plain", "a b c"]]);
    const huge = "x=1\n".repeat(highlightLimit / 4 + 1);
    expect(highlightCode("sh", huge)).toEqual([["plain", huge]]);
  });
});

describe("editing helpers", () => {
  test("Tab inserts one indent at a caret and indents every line a selection touches", () => {
    expect(applied("ab", indentEdit("ab", 1, 1))).toBe("a  []b");
    const value = "one\n\n  two\nthree";
    // From inside "one" to inside "two": three lines, the blank one stays blank.
    expect(applied(value, indentEdit(value, 1, 8))).toBe("  o[ne\n\n    t]wo\nthree");
    // A selection ending at the start of a line does not take that line.
    expect(applied(value, indentEdit(value, 0, 4))).toBe("  [one\n]\n  two\nthree");
    expect(applied(value, indentEdit(value, 0, 3, "\t"))).toBe("\t[one]\n\n  two\nthree");
  });

  test("Shift+Tab removes one indent per line, a tab or up to one unit of spaces", () => {
    const value = "    a\n b\n\tc\nd";
    expect(applied(value, outdentEdit(value, 4, 11))).toBe("  [a\nb\nc]\nd");
    expect(applied("  x", outdentEdit("  x", 1, 1))).toBe("[]x");
    expect(outdentEdit("d\ne", 0, 3)).toBeNull();
  });

  test("Enter keeps the indentation of the line being left", () => {
    expect(applied("  if x; then", newlineEdit("  if x; then", 12, 12))).toBe("  if x; then\n  []");
    expect(applied("\tfoo bar", newlineEdit("\tfoo bar", 4, 8))).toBe("\tfoo\n\t[]");
    expect(newlineEdit("plain", 5, 5).insert).toBe("\n");
    // Only the whitespace before the caret counts.
    expect(newlineEdit("    x", 2, 2).insert).toBe("\n  ");
  });

  test("a caret offset reads as a 1-based line and column", () => {
    const value = "ab\ncde\n";
    expect([0, 2, 3, 6, 7].map((offset) => caretPosition(value, offset))).toEqual([
      { line: 1, column: 1 }, { line: 1, column: 3 }, { line: 2, column: 1 }, { line: 2, column: 4 }, { line: 3, column: 1 },
    ]);
    expect([lineCount(""), lineCount("a"), lineCount("a\n"), lineCount("a\nb\nc")]).toEqual([1, 1, 2, 3]);
  });
});

describe("CodeEditor", () => {
  const render = (props) => renderToStaticMarkup(<CodeEditor aria-label="Code" onValueChange={() => {}} {...props} />);

  test("renders the text once as an editable textarea and once as highlighted paint, with a line per number", () => {
    const html = render({ value: 'echo "$HOME"\nexit 0\n', language: "sh", id: "code", name: "code" });
    expect(html).toContain('data-slot="code-editor"');
    expect(html).toContain('data-language="sh"');
    expect(html).toMatch(/<textarea[^>]*id="code"[^>]*name="code"[^>]*wrap="off"/);
    expect(html).toMatch(/<span data-token="str"[^>]*>&quot;\$HOME&quot;<\/span>/);
    expect(html).toMatch(/<span data-token="kw"[^>]*>exit<\/span>/);
    // Three lines: two of text and the empty one after the trailing newline.
    expect(html).toMatch(/data-slot="code-editor-gutter"[^>]*><div[^>]*>1\n2\n3<\/div>/);
    expect(html).toContain("Shell");
    expect(html).toContain("Ln 1, Col 1");
  });

  test("the keyboard hint is attached to the control, after any description it already has", () => {
    const html = render({ value: "", "aria-describedby": "outer-hint" });
    const describedBy = /<textarea[^>]*aria-describedby="([^"]+)"/.exec(html)[1].split(" ");
    expect(describedBy[0]).toBe("outer-hint");
    expect(html).toContain(`id="${describedBy[1]}" hidden="">Tab indents`);
    // A read-only editor makes no edits, so it promises none.
    expect(render({ value: "x", readOnly: true })).not.toContain("Tab indents");
  });

  test("an empty editor paints its placeholder, and a custom label replaces the language name", () => {
    const html = render({ value: "", placeholder: "echo hello", language: "ts", label: "triggers/src/watch.ts", lineNumbers: false });
    expect(html).toMatch(/data-slot="code-editor-paint"[^>]*><span class="text-muted-foreground">echo hello<\/span>/);
    expect(html).toContain("triggers/src/watch.ts");
    expect(html).not.toContain("TypeScript");
    expect(html).not.toContain("code-editor-gutter");
  });
});
