import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "../lib/cn";
import { HelpTooltip, type HelpPosition } from "./help-tooltip";

const buttonVariants = cva(
  "inline-flex cursor-pointer items-center justify-center gap-1.5 whitespace-nowrap rounded-md text-sm font-medium transition-all disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 outline-none focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px]",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-primary/90",
        destructive:
          "bg-destructive text-white hover:bg-destructive/90 focus-visible:ring-destructive/20",
        outline:
          "border bg-background hover:bg-accent hover:text-accent-foreground",
        secondary: "bg-secondary text-secondary-foreground hover:bg-secondary/80",
        ghost: "hover:bg-accent hover:text-accent-foreground",
        link: "text-primary underline-offset-4 hover:underline",
      },
      size: {
        default: "h-8 px-3 py-1 has-[>svg]:px-2.5",
        sm: "h-7 rounded-md px-2.5 has-[>svg]:px-2",
        lg: "h-9 rounded-md px-4 has-[>svg]:px-3",
        icon: "size-8",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

/** A potentially disabled action must always carry an explanation. */
type ButtonDisabledProps =
  | { disabled?: false; disabledReason?: never }
  | { disabled: boolean; disabledReason: string };

type ButtonProps = Omit<React.ComponentProps<"button">, "disabled" | "aria-disabled"> &
  VariantProps<typeof buttonVariants> & {
    // Use native disabled so styling, behavior, and the explanation stay in sync.
    "aria-disabled"?: never;
    disabledHelpPosition?: HelpPosition;
  } & (
    | ({ asChild?: false } & ButtonDisabledProps)
    // Arbitrary slotted elements cannot guarantee native disabled behavior.
    | { asChild: true; disabled?: false; disabledReason?: never }
  );

function Button({
  className,
  variant,
  size,
  asChild = false,
  disabled = false,
  disabledReason,
  disabledHelpPosition = "top-right",
  "aria-describedby": describedBy,
  ...props
}: ButtonProps) {
  const reasonId = React.useId();
  const reason = typeof disabledReason === "string" ? disabledReason.trim() : "";

  if (disabled && (!reason || asChild)) {
    throw new Error(
      "A disabled Button requires a non-empty disabledReason and cannot use asChild.",
    );
  }

  const Comp = asChild ? Slot : "button";
  const button = (
    <Comp
      data-slot="button"
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
      disabled={disabled}
      aria-describedby={disabled ? [describedBy, reasonId].filter(Boolean).join(" ") : describedBy}
    />
  );

  if (!disabled) return button;

  return (
    <HelpTooltip
      as="span"
      data-slot="button-disabled"
      content={reason}
      position={disabledHelpPosition}
      label="Why is this button disabled?"
      highlightOnHover
    >
      {button}
      <span id={reasonId} hidden>
        {reason}
      </span>
    </HelpTooltip>
  );
}

export { Button, buttonVariants, type ButtonProps, type ButtonDisabledProps };
