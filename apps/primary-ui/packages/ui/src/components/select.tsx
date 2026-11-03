import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { ChevronDown } from "lucide-react";

import { cn } from "../lib/cn";
import type { ChoiceOption } from "./choice-types";

const selectVariants = cva(
  "w-full min-w-0 cursor-pointer appearance-none rounded-md border border-input bg-background text-foreground text-sm outline-none transition-colors focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px] disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20",
  {
    variants: {
      size: {
        default: "h-8 py-1 pl-3 pr-8",
        sm: "h-7 py-1 pl-2.5 pr-7",
        lg: "h-9 py-1 pl-4 pr-9",
      },
    },
    defaultVariants: { size: "default" },
  },
);

type SelectProps = Omit<
  React.ComponentProps<"select">,
  "children" | "onChange" | "size" | "value" | "defaultValue" | "multiple"
> &
  VariantProps<typeof selectVariants> & {
    options: readonly ChoiceOption[];
    value?: string;
    defaultValue?: string;
    onValueChange?: (value: string) => void;
    placeholder?: string;
  };

/** A native, single-value select. Supply an associated label or aria-label. */
function Select({
  options,
  value,
  defaultValue,
  onValueChange,
  placeholder,
  size,
  className,
  ...props
}: SelectProps) {
  return (
    <span data-slot="select-wrapper" className="relative flex w-full min-w-0 items-center">
      <select
        {...props}
        data-slot="select"
        className={cn(selectVariants({ size, className }))}
        value={value}
        defaultValue={value === undefined ? (defaultValue ?? (placeholder ? "" : undefined)) : undefined}
        onChange={(event) => onValueChange?.(event.currentTarget.value)}
      >
        {placeholder !== undefined && (
          <option value="" disabled className="cursor-not-allowed">
            {placeholder}
          </option>
        )}
        {options.map((option) => (
          <option key={option.value} value={option.value} disabled={option.disabled} className="cursor-pointer disabled:cursor-not-allowed">
            {option.label}
          </option>
        ))}
      </select>
      <ChevronDown
        aria-hidden="true"
        className="pointer-events-none absolute right-2 size-3.5 text-muted-foreground"
      />
    </span>
  );
}

export { Select, selectVariants, type SelectProps };
