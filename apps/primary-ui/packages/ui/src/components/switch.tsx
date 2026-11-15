import * as React from "react";

import { cn } from "../lib/cn";

type SwitchProps = Omit<React.ComponentProps<"button">, "onChange" | "role" | "type" | "children"> & {
  checked: boolean;
  onCheckedChange?: (checked: boolean) => void;
};

/**
 * An on/off control that takes effect immediately. Controlled: the caller owns `checked`, so a
 * change that is still being saved can keep showing the last confirmed state. Supply an
 * associated label or aria-label.
 */
function Switch({ checked, onCheckedChange, className, onClick, ...props }: SwitchProps) {
  return (
    <button
      data-slot="switch"
      type="button"
      role="switch"
      aria-checked={checked}
      data-state={checked ? "on" : "off"}
      className={cn(
        "peer inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent p-0.5 outline-none transition-colors focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px] disabled:cursor-not-allowed disabled:opacity-50 motion-reduce:transition-none",
        checked ? "bg-primary" : "bg-input",
        className,
      )}
      onClick={(event) => {
        onClick?.(event);
        if (!event.defaultPrevented) onCheckedChange?.(!checked);
      }}
      {...props}
    >
      <span
        data-slot="switch-thumb"
        aria-hidden="true"
        className={cn(
          "bg-background pointer-events-none block size-4 rounded-full shadow-sm transition-transform motion-reduce:transition-none",
          checked ? "translate-x-4" : "translate-x-0",
        )}
      />
    </button>
  );
}

export { Switch, type SwitchProps };
