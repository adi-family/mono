import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "../lib/cn";

// The same box as Select, so a row of mixed controls lines up.
const fieldBox =
  "w-full min-w-0 rounded-md border border-input bg-background text-foreground text-sm outline-none transition-colors placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px] disabled:cursor-not-allowed disabled:opacity-50 read-only:bg-muted/50 read-only:text-muted-foreground aria-invalid:border-destructive aria-invalid:ring-destructive/20";

const inputVariants = cva(fieldBox, {
  variants: {
    size: {
      default: "h-8 px-3 py-1",
      sm: "h-7 px-2.5 py-1",
      lg: "h-9 px-4 py-1",
    },
    /** Machine strings — names, paths, patterns — read better in the mono face. */
    mono: { true: "font-mono", false: "" },
  },
  defaultVariants: { size: "default", mono: false },
});

type InputProps = Omit<React.ComponentProps<"input">, "size"> & VariantProps<typeof inputVariants>;

/** A single-line text input. Supply an associated label or aria-label. */
function Input({ className, size, mono, type = "text", ...props }: InputProps) {
  return <input data-slot="input" type={type} className={cn(inputVariants({ size, mono, className }))} {...props} />;
}

type TextareaProps = React.ComponentProps<"textarea"> & { mono?: boolean };

/** A multi-line text input. Supply an associated label or aria-label. */
function Textarea({ className, mono = false, rows = 3, ...props }: TextareaProps) {
  return (
    <textarea
      data-slot="textarea"
      rows={rows}
      className={cn(fieldBox, "block resize-y px-3 py-2 leading-relaxed", mono && "font-mono text-[13px]", className)}
      {...props}
    />
  );
}

export { Input, Textarea, inputVariants, type InputProps, type TextareaProps };
