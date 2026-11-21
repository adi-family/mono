import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";

import { ChatToolCall, ChatToolCode, ChatToolOutput } from "../src/components";

describe("ChatToolCode", () => {
  test("paints the code once, highlighted, as a labelled read-only region", () => {
    const html = renderToStaticMarkup(<ChatToolCode language="py">{'rows = sh("ls").lines\nlen(rows)'}</ChatToolCode>);
    expect(html).toContain('data-slot="chat-tool-code"');
    expect(html).toContain('data-language="py"');
    expect(html).toContain('aria-label="Python code"');
    expect(html).toContain('<span data-token="key"');
    expect(html).toContain('<span data-token="func"');
    expect(html).not.toContain("<textarea");
    // Every character is there exactly once, in order.
    const text = html.replace(/<[^>]+>/g, "").replaceAll("&quot;", '"');
    expect(text).toBe('rows = sh("ls").lines\nlen(rows)');
  });

  test("an unknown language is shown as it is, and a label of its own replaces the language name", () => {
    const html = renderToStaticMarkup(<ChatToolCode label="Query">{"select 1"}</ChatToolCode>);
    expect(html).toContain('aria-label="Query"');
    expect(html).toContain("<code>select 1</code>");
  });

  test("a code call carries its kind, and its code and output are both inside it", () => {
    const html = renderToStaticMarkup(
      <ChatToolCall kind="code" title="PyCode" defaultOpen>
        <ChatToolCode language="py">{"1 + 1"}</ChatToolCode>
        <ChatToolOutput>{"2"}</ChatToolOutput>
      </ChatToolCall>,
    );
    expect(html).toContain('data-kind="code"');
    expect(html.indexOf('data-slot="chat-tool-code"')).toBeLessThan(html.indexOf('data-slot="chat-tool-output"'));
  });
});
