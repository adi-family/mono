import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "../lib/cn";

const badgeVariants = cva(
  "inline-flex w-fit shrink-0 items-center gap-1.5 whitespace-nowrap rounded-md border px-1.5 py-0.5 text-xs leading-4 font-medium [&_svg]:size-3 [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        default: "border-transparent bg-primary text-primary-foreground",
        secondary: "border-transparent bg-secondary text-secondary-foreground",
        outline: "text-foreground",
        // States keep the neutral surface and carry their meaning in the dot and the word.
        success: "text-foreground [--badge-dot:var(--success)]",
        warning: "text-foreground [--badge-dot:var(--warning)]",
        destructive: "text-foreground [--badge-dot:var(--destructive)]",
        muted: "text-muted-foreground [--badge-dot:var(--muted-foreground)]",
      },
    },
    defaultVariants: { variant: "secondary" },
  },
);

type BadgeProps = React.ComponentProps<"span"> &
  VariantProps<typeof badgeVariants> & {
    /** Lead with a state dot. On by default for the state variants. */
    dot?: boolean;
  };

const stateVariants = new Set(["success", "warning", "destructive", "muted"]);

/** A short label for a kind or a state. The word says the state; the colour only repeats it. */
function Badge({ className, variant, dot, children, ...props }: BadgeProps) {
  const showDot = dot ?? stateVariants.has(variant ?? "");
  return (
    <span data-slot="badge" data-variant={variant ?? "secondary"} className={cn(badgeVariants({ variant, className }))} {...props}>
      {showDot && <span data-slot="badge-dot" aria-hidden="true" className="size-1.5 rounded-full bg-[var(--badge-dot,currentColor)]" />}
      {children}
    </span>
  );
}

export { Badge, badgeVariants, type BadgeProps };
