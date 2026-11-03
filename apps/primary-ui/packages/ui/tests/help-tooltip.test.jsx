import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";

import { HelpTooltip, QuestionMark } from "../src/components";

describe("QuestionMark", () => {
  test("renders a plain decorative mark with caller attributes", () => {
    const markup = renderToStaticMarkup(
      <QuestionMark id="field-help-mark" className="custom-mark" data-context="field" />,
    );

    expect(markup).toStartWith("<span");
    expect(markup).toContain('data-slot="question-mark"');
    expect(markup).toContain('aria-hidden="true"');
    expect(markup).toContain('id="field-help-mark"');
    expect(markup).toContain('data-context="field"');
    expect(markup).toContain("custom-mark");
    expect(markup).toEndWith("?</span>");
    expect(markup).not.toMatch(/<(?:button|a)\b|tabindex=|role="tooltip"|data-position=/);
    expect(markup).not.toContain('data-slot="help-tooltip');
  });
});

describe("HelpTooltip", () => {
  test("supports an inline span wrapper without leaking its selector prop", () => {
    const markup = renderToStaticMarkup(
      <HelpTooltip as="span" content="More details" id="inline-help">
        <span>Inline content</span>
      </HelpTooltip>,
    );

    expect(markup).toMatch(/^<span\b[^>]*data-slot="help-tooltip"/);
    expect(markup).toContain('id="inline-help"');
    expect(markup).toContain("<span>Inline content</span>");
    expect(markup).not.toMatch(/<div\b|\sas=/);
  });

  test("wraps arbitrary block content and exposes a separate accessible help button", () => {
    const markup = renderToStaticMarkup(
      <form>
        <HelpTooltip
          content={<strong>Choose a workspace before proceeding.</strong>}
          label="About this workspace"
          id="workspace-help"
          data-context="workspace"
          className="custom-help"
        >
          <section data-testid="workspace-card"><h2>Workspace</h2><p>Pending setup</p></section>
        </HelpTooltip>
      </form>,
    );

    expect(markup).toMatch(/^<form><div\b[^>]*data-slot="help-tooltip"/);
    expect(markup).toContain('data-slot="help-tooltip"');
    expect(markup).toContain('data-position="top-right"');
    expect(markup).toContain('id="workspace-help"');
    expect(markup).toContain('data-context="workspace"');
    expect(markup).toContain("custom-help");
    expect(markup).toContain('<h2>Workspace</h2><p>Pending setup</p>');
    const trigger = markup.match(/<button\b[^>]*data-slot="help-tooltip-trigger"[^>]*>/)?.[0];
    expect(trigger).toBeDefined();
    expect(trigger).toContain('type="button"');
    expect(trigger).toContain('aria-label="About this workspace"');
    expect(markup.indexOf(trigger)).toBeGreaterThan(markup.indexOf("</section>"));
    expect(markup.match(/<button\b/g)).toHaveLength(1);
  });

  test.each(["top-right", "right", "top-left", "left", "bottom-right", "bottom-left"])(
    "accepts the %s placement around an action without nesting help inside it",
    (position) => {
      const markup = renderToStaticMarkup(
        <HelpTooltip content="More details" position={position}>
          <button type="submit">Continue</button>
        </HelpTooltip>,
      );

      expect(markup).toContain(`data-position="${position}"`);
      expect(markup).toContain('<button type="submit">Continue</button>');
      expect(markup.match(/<button\b/g)).toHaveLength(2);
      const trigger = markup.match(/<button\b[^>]*data-slot="help-tooltip-trigger"[^>]*>/)?.[0];
      expect(trigger).toBeDefined();
      expect(trigger).toContain('type="button"');
      expect(markup.indexOf(trigger)).toBeGreaterThan(markup.indexOf("</button>"));
    },
  );
});
