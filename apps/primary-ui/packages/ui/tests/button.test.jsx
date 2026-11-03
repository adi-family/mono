import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";

import { Button, ThemeToggle } from "../src/components";

// JavaScript consumers can bypass the type contract, so the runtime guard must
// still reject disabled actions without usable explanations.
describe("disabled Button runtime contract", () => {
  test.each([undefined, null, "", " \t\n "])(
    "rejects an unusable disabled reason: %j",
    (disabledReason) => {
      expect(() => renderToStaticMarkup(
        <Button disabled disabledReason={disabledReason}>Continue</Button>,
      )).toThrow("non-empty disabledReason");
    },
  );

  test("rejects a missing reason", () => {
    expect(() => renderToStaticMarkup(
      <Button disabled>Continue</Button>,
    )).toThrow("non-empty disabledReason");
  });

  test("rejects disabled asChild even when a reason is supplied", () => {
    expect(() => renderToStaticMarkup(
      <Button asChild disabled disabledReason="Choose a project.">
        <a href="/">Continue</a>
      </Button>,
    )).toThrow("cannot use asChild");
  });

  test("escapes the reason and preserves existing accessible descriptions", () => {
    const markup = renderToStaticMarkup(
      <Button
        disabled
        disabledReason={'  <script>alert("x")</script> & blocked  '}
        aria-describedby="project-help account-help"
      >
        Continue
      </Button>,
    );
    const action = markup.match(/<button\b[^>]*data-slot="button"[^>]*>/)?.[0];
    expect(action).toBeDefined();
    expect(action).toContain('disabled=""');

    const descriptionIds = action.match(/aria-describedby="([^"]+)"/)?.[1].split(" ");
    expect(descriptionIds).toHaveLength(3);
    expect(descriptionIds.slice(0, 2)).toEqual(["project-help", "account-help"]);
    expect(markup).toContain(
      `<span id="${descriptionIds[2]}" hidden="">&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; blocked</span>`,
    );
    expect(markup).not.toContain("<script>");
  });

  test("renders a separate enabled help button that cannot submit its form", () => {
    const markup = renderToStaticMarkup(
      <form>
        <Button type="submit" disabled disabledReason="Choose a project.">Continue</Button>
      </form>,
    );
    const help = markup.match(/<button\b[^>]*data-slot="help-tooltip-trigger"[^>]*>/)?.[0];
    expect(help).toBeDefined();
    expect(help).toContain('type="button"');
    expect(help).toContain('aria-label="Why is this button disabled?"');
    expect(help).not.toMatch(/\sdisabled(?:=|\s|>)/);
    expect(markup).toContain('data-slot="button-disabled"');
    expect(markup).toContain('data-position="top-right"');
    expect(markup).toContain("?</span>");
    expect(markup.match(/<button\b/g)).toHaveLength(2);
    const actionEnd = markup.indexOf("</button>");
    expect(markup.indexOf(help)).toBeGreaterThan(actionEnd);
  });

  test("forwards disabled help placement without leaking the prop to the action", () => {
    const markup = renderToStaticMarkup(
      <Button disabled disabledReason="Choose a project." disabledHelpPosition="right">
        Continue
      </Button>,
    );
    expect(markup).toContain('data-position="right"');
    expect(markup).not.toMatch(/disabledhelpposition/i);
    const action = markup.match(/<button\b[^>]*data-slot="button"[^>]*>/)?.[0];
    expect(action).not.toContain("data-position");
  });

  test("keeps the disabled wrapper valid inside a paragraph", () => {
    const markup = renderToStaticMarkup(
      <p>
        Next step: <Button disabled disabledReason="Choose a project.">Continue</Button>
      </p>,
    );

    expect(markup).toMatch(/<span\b[^>]*data-slot="button-disabled"/);
    expect(markup).not.toMatch(/<div\b/);
    expect(markup).not.toMatch(/\sas=/);
    expect(markup).toStartWith("<p>Next step: ");
    expect(markup).toEndWith("</span></p>");
  });

  test("omits the helper when enabled, including a supplied dynamic-state reason", () => {
    for (const props of [{}, { disabled: false, disabledReason: "Choose a project.", disabledHelpPosition: "left" }]) {
      const markup = renderToStaticMarkup(
        <Button {...props} aria-describedby="project-help">Continue</Button>,
      );
      expect(markup).not.toContain('data-slot="button-disabled');
      expect(markup).not.toContain("Choose a project.");
      expect(markup).not.toContain('data-slot="help-tooltip');
      expect(markup).not.toMatch(/disabledhelpposition/i);
      expect(markup).toContain('aria-describedby="project-help"');
      expect(markup.match(/<button\b/g)).toHaveLength(1);
    }
  });

  test("applies the runtime contract to ThemeToggle", () => {
    expect(() => renderToStaticMarkup(
      <ThemeToggle theme="light" onThemeChange={() => {}} disabled />,
    )).toThrow("non-empty disabledReason");
    const markup = renderToStaticMarkup(
      <ThemeToggle
        theme="light"
        onThemeChange={() => {}}
        disabled
        disabledReason="Your workspace controls the theme."
        disabledHelpPosition="bottom-right"
      />,
    );
    expect(markup).toContain('data-slot="theme-toggle"');
    expect(markup).toContain('data-slot="help-tooltip-trigger"');
    expect(markup).toContain('data-position="bottom-right"');
    expect(markup).not.toMatch(/disabledhelpposition/i);
    expect(markup).toContain("Your workspace controls the theme.");
  });
});
