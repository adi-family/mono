import { Fragment, useState } from "react";
import { Check, Copy } from "lucide-react";
import { Highlight, type PrismTheme } from "prism-react-renderer";

import { Button } from "@design-system/ui";

// Token colors follow the gallery's light/dark CSS, without a second theme state.
const codeTheme: PrismTheme = { plain: {}, styles: [] };

export function CodeExample({ code, label }: { code: string; label: string }) {
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState(false);

  async function copyCode() {
    const fallbackCopy = () => {
      // The development gallery uses HTTP, where the Clipboard API may be unavailable.
      const previousFocus = document.activeElement;
      const input = document.createElement("textarea");
      input.value = code;
      input.style.position = "fixed";
      input.style.opacity = "0";
      input.setAttribute("aria-label", "Code to copy");
      document.body.append(input);
      input.select();
      try {
        if (!document.execCommand("copy")) throw new Error("Copy failed");
      } finally {
        input.remove();
        if (previousFocus instanceof HTMLElement) previousFocus.focus({ preventScroll: true });
      }
    };

    try {
      if (navigator.clipboard) {
        try {
          await navigator.clipboard.writeText(code);
        } catch {
          fallbackCopy();
        }
      } else {
        fallbackCopy();
      }
      setCopied(true);
      setCopyError(false);
    } catch {
      setCopied(false);
      setCopyError(true);
    }
  }

  return (
    <div className="code-example min-w-0 rounded-lg border">
      <div className="bg-muted/40 flex items-center justify-between gap-3 rounded-t-lg border-b px-3 py-2">
        <span className="text-muted-foreground font-mono text-xs">Usage · TSX</span>
        <Button variant="ghost" size="sm" onClick={copyCode} aria-label={`Copy ${label} example`}>
          {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
      <Highlight code={code} language="tsx" theme={codeTheme}>
        {({ tokens, getLineProps, getTokenProps }) => (
          <pre
            className="bg-muted/20 rounded-b-lg p-4 font-mono text-xs leading-6 whitespace-pre-wrap [overflow-wrap:anywhere]"
            aria-label={`${label} code example`}
          >
            <code className="language-tsx">
              {tokens.map((line, lineIndex) => (
                <Fragment key={lineIndex}>
                  <span {...getLineProps({ line })}>
                    {line.map((token, tokenIndex) => token.empty ? null : (
                      <span key={tokenIndex} {...getTokenProps({ token })} />
                    ))}
                  </span>
                  {lineIndex < tokens.length - 1 ? "\n" : null}
                </Fragment>
              ))}
            </code>
          </pre>
        )}
      </Highlight>
      <p className={copyError ? "text-muted-foreground border-t px-3 py-2 text-xs" : "sr-only"} role="status">
        {copyError ? "Clipboard unavailable here. Select and copy the code above." : copied ? "Code copied." : ""}
      </p>
    </div>
  );
}
