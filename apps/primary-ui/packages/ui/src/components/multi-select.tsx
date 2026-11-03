import * as React from "react";

import { ChoiceControl, type ChoiceControlProps } from "./choice-control";
import type { ChoiceOption } from "./choice-types";

export type MultiSelectProps = ChoiceControlProps & {
  value?: readonly ChoiceOption[];
  defaultValue?: readonly ChoiceOption[];
  onValueChange?: (options: ChoiceOption[]) => void;
};

const EMPTY: readonly ChoiceOption[] = [];

/** Searchable multiple selection; supports the same async/creation options as Autocomplete. */
export function MultiSelect({ value, defaultValue = EMPTY, onValueChange, ...props }: MultiSelectProps) {
  const [internalValue, setInternalValue] = React.useState(defaultValue);
  return <ChoiceControl {...props} multiple selected={value ?? internalValue}
    onSelectionChange={(options) => {
      if (value === undefined) setInternalValue(options);
      onValueChange?.(options);
    }}
    onReset={() => { if (value === undefined) setInternalValue(defaultValue); }}
  />;
}
