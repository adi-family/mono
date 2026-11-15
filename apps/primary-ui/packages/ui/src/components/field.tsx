import * as React from "react";

import { cn } from "../lib/cn";

/** What a Field hands its control so the label, hint and error are announced with it. */
type FieldControlProps = {
  id: string;
  "aria-describedby"?: string;
  "aria-invalid"?: true;
};

type FieldProps = Omit<React.ComponentProps<"div">, "children"> & {
  label: React.ReactNode;
  /** Standing help under the control. */
  hint?: React.ReactNode;
  /** A problem with the current value; replaces nothing, and is announced. */
  error?: React.ReactNode;
  /** Use a control id of your own instead of a generated one. */
  controlId?: string;
  children: React.ReactNode | ((control: FieldControlProps) => React.ReactNode);
};

/**
 * A labelled form control with optional help and error text. Pass a function as children to
 * receive the id and ARIA wiring for the control: `{(control) => <Input {...control} />}`.
 */
function Field({ label, hint, error, controlId, className, children, ...props }: FieldProps) {
  const generated = React.useId();
  const id = controlId ?? generated;
  const hintId = hint ? `${id}-hint` : undefined;
  const errorId = error ? `${id}-error` : undefined;
  const describedBy = [hintId, errorId].filter(Boolean).join(" ") || undefined;
  const control: FieldControlProps = { id, ...(describedBy ? { "aria-describedby": describedBy } : {}), ...(error ? { "aria-invalid": true as const } : {}) };

  return (
    <div data-slot="field" className={cn("flex min-w-0 flex-col gap-1.5", className)} {...props}>
      <label data-slot="field-label" htmlFor={id} className="text-sm leading-none font-medium">
        {label}
      </label>
      {typeof children === "function" ? children(control) : children}
      {hint && (
        <p data-slot="field-hint" id={hintId} className="text-muted-foreground text-xs leading-relaxed">
          {hint}
        </p>
      )}
      {error && (
        <p data-slot="field-error" id={errorId} role="alert" className="text-destructive text-xs leading-relaxed">
          {error}
        </p>
      )}
    </div>
  );
}

export { Field, type FieldProps, type FieldControlProps };
