import * as React from "react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "../lib/cn";
import { QuestionMark } from "./question-mark";

const helpTriggerVariants = cva(
  "z-10 inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-background text-muted-foreground outline-none transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
  {
    variants: {
      position: {
        "top-right": "absolute top-0 right-0 translate-x-1/2 -translate-y-1/2",
        right: "relative",
        "top-left": "absolute top-0 left-0 -translate-x-1/2 -translate-y-1/2",
        left: "relative",
        "bottom-right": "absolute right-0 bottom-0 translate-x-1/2 translate-y-1/2",
        "bottom-left": "absolute bottom-0 left-0 -translate-x-1/2 translate-y-1/2",
      },
    },
    defaultVariants: { position: "top-right" },
  },
);

type HelpPosition = NonNullable<VariantProps<typeof helpTriggerVariants>["position"]>;

type HelpTooltipProps = (
  | ({ as?: "div" } & Omit<React.ComponentProps<"div">, "content" | "children">)
  | ({ as: "span" } & Omit<React.ComponentProps<"span">, "content" | "children">)
) & {
  children: React.ReactNode;
  content: React.ReactNode;
  /** Placement of the question mark relative to the wrapped content. */
  position?: HelpPosition;
  /** Preferred popup direction; it can flip to stay inside the viewport. */
  side?: React.ComponentProps<typeof Tooltip.Content>["side"];
  align?: React.ComponentProps<typeof Tooltip.Content>["align"];
  label?: string;
  /** Draw attention to the help when hovering the wrapped content. */
  highlightOnHover?: boolean;
};

function HelpTooltip({
  children,
  content,
  position = "top-right",
  side = "top",
  align = "center",
  label = "More information",
  highlightOnHover = false,
  className,
  ...props
}: HelpTooltipProps) {
  const [open, setOpen] = React.useState(false);

  const wrapperClassName = cn(
    "group/help relative inline-flex w-fit items-center gap-1.5 align-middle",
    position === "left" && "flex-row-reverse",
    className,
  );
  const wrapperContent = (
    <>
      {children}
      <Tooltip.Provider delayDuration={200}>
        <Tooltip.Root open={open} onOpenChange={setOpen}>
          <Tooltip.Trigger
            type="button"
            data-slot="help-tooltip-trigger"
            aria-label={label}
            className={cn(
              helpTriggerVariants({ position }),
              highlightOnHover &&
                "group-hover/help:bg-accent group-hover/help:text-accent-foreground group-hover/help:ring-2 group-hover/help:ring-ring/50",
            )}
            onClick={(event) => {
              // Focus and hover also open it; a tap must work independently of hover.
              event.preventDefault();
              setOpen(true);
            }}
          >
            <span className="relative inline-flex size-4" aria-hidden="true">
              {highlightOnHover && (
                <>
                  <span
                    data-slot="help-tooltip-wave"
                    className="pointer-events-none absolute inset-0 rounded-full border border-current opacity-0 motion-safe:group-hover/help:animate-ping motion-safe:group-hover/help:opacity-40 motion-safe:group-hover/help:[animation-duration:1.6s]"
                  />
                  <span
                    data-slot="help-tooltip-wave"
                    className="pointer-events-none absolute inset-0 rounded-full border border-current opacity-0 motion-safe:group-hover/help:animate-ping motion-safe:group-hover/help:opacity-25 motion-safe:group-hover/help:[animation-delay:800ms] motion-safe:group-hover/help:[animation-duration:1.6s]"
                  />
                </>
              )}
              <QuestionMark />
            </span>
          </Tooltip.Trigger>
          <Tooltip.Portal>
            <Tooltip.Content
              data-slot="help-tooltip-content"
              side={side}
              align={align}
              sideOffset={6}
              collisionPadding={12}
              className="z-50 max-w-[min(20rem,calc(100vw-1.5rem))] rounded-md border bg-popover px-3 py-2 text-sm text-popover-foreground wrap-break-word"
            >
              {content}
              <Tooltip.Arrow className="fill-popover" />
            </Tooltip.Content>
          </Tooltip.Portal>
        </Tooltip.Root>
      </Tooltip.Provider>
    </>
  );

  // Keep Button valid in inline text, while allowing arbitrary blocks by default.
  if (props.as === "span") {
    const { as: _as, ...wrapperProps } = props;
    return (
      <span data-slot="help-tooltip" data-position={position} className={wrapperClassName} {...wrapperProps}>
        {wrapperContent}
      </span>
    );
  }

  const { as: _as, ...wrapperProps } = props;
  return (
    <div data-slot="help-tooltip" data-position={position} className={wrapperClassName} {...wrapperProps}>
      {wrapperContent}
    </div>
  );
}

export { HelpTooltip, type HelpTooltipProps, type HelpPosition };
