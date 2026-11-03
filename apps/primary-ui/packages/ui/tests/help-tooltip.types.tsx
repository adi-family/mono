import type * as React from "react";

import { Button, HelpTooltip, QuestionMark, ThemeToggle } from "../src/components";

export function acceptedHelpExamples() {
  const spanRef: React.RefObject<HTMLSpanElement | null> = { current: null };
  const divRef: React.RefObject<HTMLDivElement | null> = { current: null };
  const markProps: React.ComponentProps<typeof QuestionMark> = {
    id: "field-help-mark",
    className: "custom-mark",
    title: "Help",
  };

  return (
    <>
      <QuestionMark {...markProps} />
      <HelpTooltip content="A useful explanation"><button>Continue</button></HelpTooltip>
      <HelpTooltip content="Details" ref={divRef}><div>Block</div></HelpTooltip>
      <HelpTooltip as="div" content="Details" ref={divRef}><div>Block</div></HelpTooltip>
      <HelpTooltip as="span" content="Details" ref={spanRef}><span>Inline content</span></HelpTooltip>
      <HelpTooltip content={<strong>Details</strong>} position="top-right"><div>Block</div></HelpTooltip>
      <HelpTooltip content="Details" position="right"><div>Block</div></HelpTooltip>
      <HelpTooltip content="Details" position="top-left"><div>Block</div></HelpTooltip>
      <HelpTooltip content="Details" position="left"><div>Block</div></HelpTooltip>
      <HelpTooltip content="Details" position="bottom-right"><div>Block</div></HelpTooltip>
      <HelpTooltip content="Details" position="bottom-left"><div>Block</div></HelpTooltip>
      <HelpTooltip
        content="Details"
        side="left"
        align="start"
        label="About this block"
        highlightOnHover
        className="custom-help"
        id="block-help"
        onMouseEnter={() => {}}
      >
        <section>Block</section>
      </HelpTooltip>
      <Button disabled disabledReason="Choose a project." disabledHelpPosition="top-left">Continue</Button>
      <ThemeToggle
        theme="light"
        onThemeChange={() => {}}
        disabled
        disabledReason="Workspace setting."
        disabledHelpPosition="left"
      />
    </>
  );
}

export function rejectedHelpExamples() {
  // @ts-expect-error The helper wrapper must not create a nested interactive element.
  <HelpTooltip as="button" content="More details"><span>Inline content</span></HelpTooltip>;
  // @ts-expect-error The plain mark has no positioning behavior.
  <QuestionMark position="top-right" />;
  // @ts-expect-error The plain mark has no tooltip placement behavior.
  <QuestionMark side="top" />;
  // @ts-expect-error A tooltip needs an explanation.
  <HelpTooltip><div>Block</div></HelpTooltip>;
  // @ts-expect-error A positioned helper needs something to wrap.
  <HelpTooltip content="More details" />;
  // @ts-expect-error Placement is limited to supported anchor positions.
  <HelpTooltip content="More details" position="diagonal"><div>Block</div></HelpTooltip>;
  // @ts-expect-error Tooltip side must be a supported Radix side.
  <HelpTooltip content="More details" side="diagonal"><div>Block</div></HelpTooltip>;
  // @ts-expect-error Tooltip alignment must be a supported Radix alignment.
  <HelpTooltip content="More details" align="left"><div>Block</div></HelpTooltip>;
  // @ts-expect-error Button shares the supported helper placements.
  <Button disabled disabledReason="Choose a project." disabledHelpPosition="diagonal">Continue</Button>;
  // @ts-expect-error ThemeToggle also preserves the helper placement contract.
  <ThemeToggle theme="light" onThemeChange={() => {}} disabled disabledReason="Workspace setting." disabledHelpPosition="diagonal" />;
}
