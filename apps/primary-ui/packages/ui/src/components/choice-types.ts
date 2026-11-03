/** A stable value and human-readable label shared by all choice controls. */
export type ChoiceOption = {
  value: string;
  label: string;
  disabled?: boolean;
  /** Additional search terms for Autocomplete and MultiSelect. */
  keywords?: readonly string[];
};
