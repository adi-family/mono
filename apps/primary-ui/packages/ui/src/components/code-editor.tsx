import * as React from "react";

import { cn } from "../lib/cn";
import { caretPosition, indentEdit, lineCount, newlineEdit, outdentEdit, type CodeEdit } from "../lib/code-edit";
import { codeLanguageLabels, highlightCode, type CodeLanguage, type CodeToken } from "../lib/highlight";

/**
 * Everything that decides where a glyph lands, applied to the painted layer, the textarea and
 * the line numbers alike. They have to agree on font, size, line height, padding and tab size to
 * the pixel: any divergence shows up as the caret drifting away from the text it is inside.
 * Lines never wrap, so a line number always sits beside its own line.
 */
const layer = "m-0 border-0 py-3 font-mono text-[13px] leading-5 whitespace-pre [tab-size:2]";
const lineHeight = 20;
const verticalPadding = 24;
const headerHeight = 32;

/**
 * Ink per token. Colour only — never a weight or a style, which would change glyph advances
 * and pull the paint out from under the caret.
 */
const ink: Record<CodeToken, string> = {
  plain: "",
  comment: "text-muted-foreground",
  str: "text-emerald-700 dark:text-emerald-300",
  num: "text-orange-700 dark:text-orange-300",
  key: "text-sky-700 dark:text-sky-300",
  kw: "text-violet-700 dark:text-violet-300",
  func: "text-blue-700 dark:text-blue-300",
  punct: "text-muted-foreground",
};

type CodeEditorProps = Omit<React.ComponentProps<"textarea">, "value" | "defaultValue" | "onChange" | "rows" | "wrap" | "children"> & {
  value: string;
  onValueChange?: (value: string) => void;
  /** What to scan the text as. It may change while the editor stays mounted. */
  language?: CodeLanguage;
  /** Lines visible before the editor scrolls; the box can also be dragged taller. */
  rows?: number;
  /** Shown in the header: the language's name by default, or a file's path. */
  label?: React.ReactNode;
  lineNumbers?: boolean;
  /** How many spaces Tab inserts. */
  indentSize?: number;
};

/**
 * A code editor: a highlighted view of the text with a real textarea on top of it.
 *
 * Two layers share one box — the painted code underneath and a transparent-text textarea above
 * it that still owns the caret, the selection, undo, paste and the input method. The browser
 * does the editing; the paint only has to keep up, which it does by following the textarea's
 * scroll. Tab indents and Shift+Tab outdents; Escape then Tab leaves the editor, so the keyboard
 * is never trapped. Supply an associated label or aria-label.
 */
function CodeEditor({
  value,
  onValueChange,
  language = "plain",
  rows = 16,
  label,
  lineNumbers = true,
  indentSize = 2,
  className,
  style,
  readOnly,
  placeholder,
  "aria-describedby": describedBy,
  onKeyDown,
  onScroll,
  onSelect,
  onBlur,
  ...props
}: CodeEditorProps) {
  const area = React.useRef<HTMLTextAreaElement>(null);
  const paint = React.useRef<HTMLPreElement>(null);
  const gutter = React.useRef<HTMLDivElement>(null);
  // Escape arms the next Tab to move focus instead of indenting.
  const leaving = React.useRef(false);
  const [caret, setCaret] = React.useState({ line: 1, column: 1 });
  const hintId = React.useId();
  const runs = React.useMemo(() => highlightCode(language, value), [language, value]);
  const lines = React.useMemo(() => lineCount(value), [value]);
  const unit = " ".repeat(indentSize);

  // The textarea is the one that scrolls; the paint and the line numbers are moved to match.
  const follow = React.useCallback(() => {
    const element = area.current;
    if (!element) return;
    if (paint.current) paint.current.style.transform = `translate(${-element.scrollLeft}px, ${-element.scrollTop}px)`;
    if (gutter.current) gutter.current.style.transform = `translateY(${-element.scrollTop}px)`;
  }, []);
  // A change of text can move the scroll position without a scroll event.
  React.useLayoutEffect(follow, [follow, value]);

  function locate() {
    const element = area.current;
    if (!element) return;
    const next = caretPosition(element.value, element.selectionDirection === "backward" ? element.selectionStart : element.selectionEnd);
    setCaret((current) => (current.line === next.line && current.column === next.column ? current : next));
  }

  /** Make an edit the way typing would, so the browser's own undo history keeps it. */
  function apply(edit: CodeEdit) {
    const element = area.current;
    if (!element) return;
    element.setSelectionRange(edit.from, edit.to);
    const typed = edit.insert
      ? document.execCommand("insertText", false, edit.insert)
      : edit.from === edit.to || document.execCommand("delete");
    if (!typed) {
      // Without the editing command the replacement still happens; only its undo step is lost.
      element.setRangeText(edit.insert, edit.from, edit.to, "end");
      element.dispatchEvent(new Event("input", { bubbles: true }));
    }
    element.setSelectionRange(edit.selectionStart, edit.selectionEnd);
    locate();
  }

  return (
    <div
      data-slot="code-editor"
      data-language={language}
      className={cn(
        "border-input bg-background focus-within:border-ring focus-within:ring-ring/50 has-[textarea[aria-invalid=true]]:border-destructive flex max-h-[85svh] min-h-28 w-full min-w-0 resize-y flex-col overflow-hidden rounded-md border transition-colors focus-within:ring-[3px]",
        readOnly && "bg-muted/50",
        className,
      )}
      style={{ height: headerHeight + rows * lineHeight + verticalPadding + 2, ...style }}
    >
      <div data-slot="code-editor-header" className="text-muted-foreground flex h-8 shrink-0 items-center justify-between gap-3 border-b px-3 text-xs">
        <span data-slot="code-editor-label" className="min-w-0 truncate">{label ?? codeLanguageLabels[language]}</span>
        <span data-slot="code-editor-position" aria-hidden="true" className="shrink-0 tabular-nums">
          Ln {caret.line}, Col {caret.column}
        </span>
      </div>
      <div className="flex min-h-0 flex-1">
        {lineNumbers && (
          <div data-slot="code-editor-gutter" aria-hidden="true" className="bg-muted/40 text-muted-foreground/70 shrink-0 overflow-hidden border-r text-right select-none">
            <div ref={gutter} className={cn(layer, "px-2.5 tabular-nums")} style={{ minWidth: `calc(${Math.max(2, String(lines).length)}ch + 1.25rem)` }}>
              {Array.from({ length: lines }, (_, index) => index + 1).join("\n")}
            </div>
          </div>
        )}
        <div className="relative min-w-0 flex-1 overflow-hidden">
          <pre ref={paint} data-slot="code-editor-paint" aria-hidden="true" className={cn(layer, "text-foreground pointer-events-none absolute top-0 left-0 w-max min-w-full px-3")}>
            {value === "" && placeholder ? <span className="text-muted-foreground">{placeholder}</span> : runs.map(([token, text], index) => (
              token === "plain" ? text : <span key={index} data-token={token} className={ink[token]}>{text}</span>
            ))}
            {/* A trailing newline paints as nothing; a space gives the caret's last line something to be. */}
            {value.endsWith("\n") ? " " : null}
          </pre>
          {/* Above the paint, and invisible: the glyphs come from the layer underneath, but the
              caret, the selection and every editing behaviour a browser has are this element's. */}
          <textarea
            {...props}
            ref={area}
            data-slot="code-editor-input"
            className={cn(layer, "caret-foreground selection:bg-foreground/20 relative block size-full resize-none overflow-auto bg-transparent px-3 text-transparent outline-none [-webkit-text-fill-color:transparent] selection:text-transparent")}
            wrap="off"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            autoComplete="off"
            readOnly={readOnly}
            aria-describedby={[describedBy, readOnly ? null : hintId].filter(Boolean).join(" ") || undefined}
            value={value}
            onChange={(event) => onValueChange?.(event.target.value)}
            onScroll={(event) => { follow(); onScroll?.(event); }}
            onSelect={(event) => { locate(); onSelect?.(event); }}
            onBlur={(event) => { leaving.current = false; onBlur?.(event); }}
            onKeyDown={(event) => {
              onKeyDown?.(event);
              if (event.defaultPrevented || event.nativeEvent.isComposing || readOnly) return;
              const plain = !event.altKey && !event.ctrlKey && !event.metaKey;
              // The element's own text: the caret offsets are measured against it.
              const { selectionStart, selectionEnd, value: text } = event.currentTarget;
              if (event.key === "Escape") {
                leaving.current = true;
              } else if (event.key === "Tab" && plain) {
                if (leaving.current) { leaving.current = false; return; }
                event.preventDefault();
                const edit = event.shiftKey ? outdentEdit(text, selectionStart, selectionEnd, unit) : indentEdit(text, selectionStart, selectionEnd, unit);
                if (edit) apply(edit);
              } else if (event.key === "Enter" && plain && !event.shiftKey) {
                const edit = newlineEdit(text, selectionStart, selectionEnd);
                // A line with no indentation is the browser's own Enter.
                if (edit.insert !== "\n") { event.preventDefault(); apply(edit); }
                leaving.current = false;
              } else if (!["Shift", "Control", "Alt", "Meta"].includes(event.key)) {
                leaving.current = false;
              }
            }}
          />
        </div>
      </div>
      {!readOnly && <span id={hintId} hidden>Tab indents and Shift+Tab outdents. Press Escape, then Tab, to move to the next control.</span>}
    </div>
  );
}

export { CodeEditor, type CodeEditorProps };
