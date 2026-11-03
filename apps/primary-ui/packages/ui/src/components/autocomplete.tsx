import * as React from "react";

import { ChoiceControl, type ChoiceControlProps } from "./choice-control";
import type { ChoiceOption } from "./choice-types";

export type AutocompleteProps = ChoiceControlProps & {
  value?: ChoiceOption | null;
  defaultValue?: ChoiceOption | null;
  onValueChange?: (option: ChoiceOption | null) => void;
};

/** Single selection with local or async search and optional option creation. */
export function Autocomplete({ value, defaultValue = null, onValueChange, ...props }: AutocompleteProps) {
  const [internalValue, setInternalValue] = React.useState(defaultValue);
  const selected = value === undefined ? internalValue : value;
  return <ChoiceControl {...props} multiple={false} selected={selected ? [selected] : []}
    onSelectionChange={(options) => {
      const next = options[0] ?? null;
      if (value === undefined) setInternalValue(next);
      onValueChange?.(next);
    }}
    onReset={() => { if (value === undefined) setInternalValue(defaultValue); }}
  />;
}
