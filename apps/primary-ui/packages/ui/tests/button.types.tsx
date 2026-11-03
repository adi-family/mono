import type * as React from "react";

import {
  Button,
  ThemeToggle,
  type ButtonDisabledProps,
  type ButtonProps,
  type ThemeToggleProps,
} from "../src/components";

// Compile-only contract tests, included in `bun run typecheck`. An unused
// expectation directive fails the check if a forbidden combination becomes accepted.
const reason = "Choose a project before continuing.";
const onThemeChange = () => {};

function ButtonWrapper(props: React.ComponentProps<typeof Button>) {
  return <Button {...props} />;
}

function ThemeToggleWrapper(props: ThemeToggleProps) {
  return <ThemeToggle {...props} />;
}

export function acceptedButtonExamples(disabled: boolean) {
  const state: ButtonDisabledProps = { disabled, disabledReason: reason };
  const props: ButtonProps = { ...state, variant: "outline", type: "submit" };
  const inferredProps: React.ComponentProps<typeof Button> = {
    disabled: true,
    disabledReason: reason,
  };
  const themeProps: ThemeToggleProps = {
    theme: "light",
    onThemeChange,
    disabled,
    disabledReason: reason,
  };

  return (
    <>
      <Button>Continue</Button>
      <Button disabled={false}>Continue</Button>
      <Button disabled={false} disabledReason={reason}>Continue</Button>
      <Button disabled disabledReason={reason}>Continue</Button>
      <Button disabled={disabled} disabledReason={reason}>Continue</Button>
      <Button {...props}>Continue</Button>
      <Button {...inferredProps}>Continue</Button>
      <Button asChild><a href="/">Home</a></Button>
      <Button asChild disabled={false}><a href="/">Home</a></Button>
      <ButtonWrapper disabled={disabled} disabledReason={reason} />
      <ThemeToggle theme="light" onThemeChange={onThemeChange} />
      <ThemeToggle theme="dark" onThemeChange={onThemeChange} disabled={false} />
      <ThemeToggle theme="light" onThemeChange={onThemeChange} disabled disabledReason={reason} />
      <ThemeToggle {...themeProps} />
      <ThemeToggleWrapper {...themeProps} />
    </>
  );
}

export function rejectedButtonExamples(disabled: boolean) {
  // @ts-expect-error A disabled button requires a reason.
  <Button disabled>Continue</Button>;
  // @ts-expect-error A dynamic boolean may disable the button, so it needs a reason.
  <Button disabled={disabled}>Continue</Button>;
  // @ts-expect-error Undefined is not an explanation.
  <Button disabled disabledReason={undefined}>Continue</Button>;
  // @ts-expect-error Null is not an explanation.
  <Button disabled disabledReason={null}>Continue</Button>;
  // @ts-expect-error Explanations must be text, not arbitrary React nodes.
  <Button disabled disabledReason={<span>{reason}</span>}>Continue</Button>;
  // @ts-expect-error aria-disabled must not bypass the explanation requirement.
  <Button aria-disabled>Continue</Button>;
  // @ts-expect-error Even with a reason, use disabled instead of aria-disabled.
  <Button aria-disabled="true" disabledReason={reason}>Continue</Button>;
  // @ts-expect-error asChild cannot guarantee native disabled behavior.
  <Button asChild disabled disabledReason={reason}><a href="/">Home</a></Button>;
  // @ts-expect-error A dynamic disabled value cannot be used with asChild either.
  <Button asChild disabled={disabled} disabledReason={reason}><a href="/">Home</a></Button>;
  // @ts-expect-error A possibly slotted element cannot guarantee disabled behavior.
  <Button asChild={disabled} disabled disabledReason={reason}>Continue</Button>;

  const propsWithoutReason = { disabled };
  // @ts-expect-error Spreading props must preserve the explanation requirement.
  <Button {...propsWithoutReason}>Continue</Button>;
  const ariaDisabledProps = { "aria-disabled": true } as const;
  // @ts-expect-error Spreading ARIA props cannot bypass the requirement.
  <Button {...ariaDisabledProps}>Continue</Button>;
  // @ts-expect-error ComponentProps-derived wrappers retain the contract.
  <ButtonWrapper disabled>Continue</ButtonWrapper>;

  // @ts-expect-error The exported state contract requires a reason.
  const invalidState: ButtonDisabledProps = { disabled: true };
  // @ts-expect-error The exported component props require a reason.
  const invalidProps: ButtonProps = { disabled };
  // @ts-expect-error React's inferred component props retain the contract.
  const invalidInferredProps: React.ComponentProps<typeof Button> = { disabled: true };

  // @ts-expect-error ThemeToggle must preserve Button's disabled contract.
  <ThemeToggle theme="light" onThemeChange={onThemeChange} disabled />;
  // @ts-expect-error ThemeToggle also requires a reason for a dynamic state.
  <ThemeToggle theme="light" onThemeChange={onThemeChange} disabled={disabled} />;
  // @ts-expect-error Undefined is not an explanation for ThemeToggle.
  <ThemeToggle theme="light" onThemeChange={onThemeChange} disabled disabledReason={undefined} />;
  // @ts-expect-error ThemeToggle must not expose the aria-disabled bypass.
  <ThemeToggle theme="light" onThemeChange={onThemeChange} aria-disabled />;
  // @ts-expect-error ThemeToggle prop spreads retain the contract.
  <ThemeToggle theme="light" onThemeChange={onThemeChange} {...propsWithoutReason} />;
  // @ts-expect-error ThemeToggle wrappers retain the contract.
  <ThemeToggleWrapper theme="light" onThemeChange={onThemeChange} disabled />;
  // @ts-expect-error The exported ThemeToggle props retain the contract.
  const invalidThemeProps: ThemeToggleProps = { theme: "light", onThemeChange, disabled };
  // @ts-expect-error React's inferred ThemeToggle props retain the contract.
  const invalidInferredThemeProps: React.ComponentProps<typeof ThemeToggle> = {
    theme: "light", onThemeChange, disabled: true,
  };

  return { invalidState, invalidProps, invalidInferredProps, invalidThemeProps, invalidInferredThemeProps };
}
