import type * as React from "react";

import { cn } from "../lib/cn";

type QuestionMarkProps = Omit<React.ComponentProps<"span">, "children">;

/** A decorative question mark with no interaction or positioning of its own. */
function QuestionMark({ className, ...props }: QuestionMarkProps) {
  return (
    <span
      data-slot="question-mark"
      aria-hidden="true"
      className={cn(
        "inline-flex size-4 shrink-0 items-center justify-center rounded-full border border-current text-xs leading-none font-semibold",
        className,
      )}
      {...props}
    >
      ?
    </span>
  );
}

export { QuestionMark, type QuestionMarkProps };
