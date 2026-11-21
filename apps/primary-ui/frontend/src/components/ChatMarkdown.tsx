import { memo, useEffect, useId, useRef, useState, type ComponentProps } from "react";
import { Check, Copy, X } from "lucide-react";
import Markdown, { type Components, type ExtraProps } from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";

import { Button } from "@design-system/ui";
import { copyText } from "../lib/copy-text";

/**
 * A fenced block with a copy button at its top-right corner. The button belongs to the frame
 * around the block, not to the block, so it stays in the corner while the code scrolls.
 */
function CodeBlock({ node: _node, ...props }: ComponentProps<"pre"> & ExtraProps) {
  const block = useRef<HTMLPreElement>(null);
  const [outcome, setOutcome] = useState<"copied" | "failed" | null>(null);
  useEffect(() => {
    if (!outcome) return;
    const timer = window.setTimeout(() => setOutcome(null), 2000);
    return () => window.clearTimeout(timer);
  }, [outcome]);
  async function copy() {
    // The line break that ends the fence is not code: pasted into a shell it would run the command.
    const code = (block.current?.textContent ?? "").replace(/\n$/, "");
    setOutcome(await copyText(code) ? "copied" : "failed");
  }
  const label = outcome === "copied" ? "Copied" : outcome === "failed" ? "Could not copy" : "Copy code";
  return (
    <div data-slot="chat-markdown-code" className="group/code relative">
      <pre {...props} ref={block} tabIndex={0} role="region" aria-label="Code block" />
      <Button
        type="button" size="icon" variant="outline" aria-label="Copy code" title={label}
        data-slot="chat-markdown-code-copy" data-outcome={outcome ?? undefined}
        className="text-muted-foreground absolute top-[9px] right-[9px] size-7 opacity-0 transition-opacity group-focus-within/code:opacity-100 group-hover/code:opacity-100 pointer-coarse:opacity-100"
        onClick={() => void copy()}
      >
        {outcome === "copied" ? <Check aria-hidden="true" className="size-3.5" />
          : outcome === "failed" ? <X aria-hidden="true" className="text-destructive size-3.5" />
            : <Copy aria-hidden="true" className="size-3.5" />}
      </Button>
      <span role="status" className="sr-only">
        {outcome === "copied" ? "Code copied" : outcome === "failed" ? "Could not copy the code. Select it and copy it by hand." : ""}
      </span>
    </div>
  );
}

const components: Components = {
  a: ({ node: _node, href, children, ...props }) => href ? (
    <a {...props} href={href} target={href.startsWith("#") ? undefined : "_blank"} rel={href.startsWith("#") ? undefined : "noopener noreferrer"}>
      {children}
    </a>
  ) : <span>{children}</span>,
  pre: CodeBlock,
  // Sideways overscroll stays inside the table; a vertical wheel turn must still reach the transcript.
  table: ({ node: _node, ...props }) => (
    <div data-slot="chat-markdown-table" tabIndex={0} role="region" aria-label="Table" className="max-w-full overflow-x-auto overscroll-x-contain rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring/50">
      <table {...props} />
    </div>
  ),
  img: ({ node: _node, src, alt, ...props }) => src ? <img {...props} src={src} alt={alt ?? ""} loading="lazy" /> : <span>{alt}</span>,
};

/** Message text only; execution output and tool arguments keep their literal formatting. */
export const ChatMarkdown = memo(function ChatMarkdown({ children }: { children: string }) {
  const id = useId();
  return (
    <div data-slot="chat-markdown" className="chat-markdown min-w-0 whitespace-normal [overflow-wrap:anywhere]">
      <Markdown
        remarkPlugins={[remarkGfm, remarkBreaks]}
        remarkRehypeOptions={{ clobberPrefix: `message-${id.replace(/:/g, "")}-` }}
        components={components}
      >
        {children}
      </Markdown>
    </div>
  );
});
