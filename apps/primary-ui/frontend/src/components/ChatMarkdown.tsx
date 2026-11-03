import { memo, useId } from "react";
import Markdown, { type Components } from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";

const components: Components = {
  a: ({ node: _node, href, children, ...props }) => href ? (
    <a {...props} href={href} target={href.startsWith("#") ? undefined : "_blank"} rel={href.startsWith("#") ? undefined : "noopener noreferrer"}>
      {children}
    </a>
  ) : <span>{children}</span>,
  pre: ({ node: _node, ...props }) => <pre {...props} tabIndex={0} role="region" aria-label="Code block" />,
  table: ({ node: _node, ...props }) => (
    <div data-slot="chat-markdown-table" tabIndex={0} role="region" aria-label="Table" className="max-w-full overflow-x-auto overscroll-contain rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring/50">
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
