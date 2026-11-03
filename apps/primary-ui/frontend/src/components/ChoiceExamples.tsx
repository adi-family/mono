import { useState, type ReactNode } from "react";

import {
  Autocomplete,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  MultiSelect,
  Select,
  type ChoiceOption,
} from "@design-system/ui";

import { CodeExample } from "./CodeExample";

const frameworks: ChoiceOption[] = [
  { value: "react", label: "React", keywords: ["jsx"] },
  { value: "vue", label: "Vue" },
  { value: "svelte", label: "Svelte" },
  { value: "angular", label: "Angular", disabled: true },
  { value: "solid", label: "Solid" },
];

function newOption(input: string): ChoiceOption {
  const label = input.trim();
  return { value: `custom:${label.toLowerCase().replace(/\s+/g, "-")}`, label };
}

// Each example gets an independent, in-memory server. Reload the page to reset it.
function createDemoSource() {
  let failedOnce = false;
  const records = [...frameworks];

  return {
    async loadOptions(query: string, { signal }: { signal: AbortSignal }) {
      await new Promise<void>((resolve, reject) => {
        if (signal.aborted) {
          reject(new DOMException("Aborted", "AbortError"));
          return;
        }
        const abort = () => {
          window.clearTimeout(timer);
          reject(new DOMException("Aborted", "AbortError"));
        };
        const timer = window.setTimeout(() => {
          signal.removeEventListener("abort", abort);
          resolve();
        }, 650);
        signal.addEventListener("abort", abort, { once: true });
      });

      const search = query.trim().toLowerCase();
      if (search === "error") {
        if (!failedOnce) {
          failedOnce = true;
          throw new Error("Demo request failed. Retry to reconnect.");
        }
        return records;
      }
      return records.filter((option) =>
        [option.label, ...(option.keywords ?? [])].some((text) => text.toLowerCase().includes(search)),
      );
    },
    async onCreate(input: string) {
      await new Promise((resolve) => window.setTimeout(resolve, 450));
      const option = newOption(input);
      records.push(option);
      return option;
    },
  };
}

const remoteAutocomplete = createDemoSource();
const remoteCreatable = createDemoSource();
const remoteMultiSelect = createDemoSource();

const optionsCode = `const options: ChoiceOption[] = [
  { value: "react", label: "React", keywords: ["jsx"] },
  { value: "vue", label: "Vue" },
  { value: "svelte", label: "Svelte" },
  { value: "angular", label: "Angular", disabled: true },
  { value: "solid", label: "Solid" },
];`;

const selectCode = `${optionsCode}
const [value, setValue] = useState("");

<label htmlFor="framework">Framework</label>
<Select
  id="framework"
  name="framework"
  options={options}
  value={value}
  onValueChange={setValue}
  placeholder="Choose a framework"
/>`;

const multiSelectCode = `${optionsCode}
const [value, setValue] = useState<ChoiceOption[]>([options[0]]);

<label htmlFor="frameworks">Frameworks</label>
<MultiSelect
  id="frameworks"
  name="frameworks"
  options={options}
  value={value}
  onValueChange={setValue}
  placeholder="Add frameworks"
/>`;

const localAutocompleteCode = `${optionsCode}
const [value, setValue] = useState<ChoiceOption | null>(null);

<label htmlFor="framework">Framework</label>
<Autocomplete
  id="framework"
  options={options}
  value={value}
  onValueChange={setValue}
  placeholder="Search frameworks"
/>`;

const asyncAutocompleteCode = `const [value, setValue] = useState<ChoiceOption | null>(null);
// Your loader: loadOptions(query, { signal }) => Promise<ChoiceOption[]>

<label htmlFor="framework">Remote framework</label>
<Autocomplete
  id="framework"
  loadOptions={loadOptions}
  debounceMs={250}
  value={value}
  onValueChange={setValue}
  placeholder="Search remote frameworks"
/>`;

const creatableAutocompleteCode = `${optionsCode}
const [value, setValue] = useState<ChoiceOption | null>(null);
// onCreate(input) => ChoiceOption; save the new option to your list.

<label htmlFor="framework">Framework</label>
<Autocomplete
  id="framework"
  options={options}
  value={value}
  onValueChange={setValue}
  creatable
  onCreate={onCreate}
  placeholder="Search or create a framework"
/>`;

const asyncCreatableCode = `const [value, setValue] = useState<ChoiceOption | null>(null);
// loadOptions(query, { signal }) => Promise<ChoiceOption[]>
// onCreate(input) => Promise<ChoiceOption>; save to your server.

<label htmlFor="framework">Remote framework</label>
<Autocomplete
  id="framework"
  loadOptions={loadOptions}
  value={value}
  onValueChange={setValue}
  creatable
  onCreate={onCreate}
  placeholder="Search or create remotely"
/>`;

const asyncMultiSelectCode = `const [value, setValue] = useState<ChoiceOption[]>([]);
// loadOptions(query, { signal }) => Promise<ChoiceOption[]>
// onCreate(input) => Promise<ChoiceOption>; save to your server.

<label htmlFor="frameworks">Remote frameworks</label>
<MultiSelect
  id="frameworks"
  name="frameworks"
  loadOptions={loadOptions}
  value={value}
  onValueChange={setValue}
  creatable
  onCreate={onCreate}
  placeholder="Search or create frameworks"
/>`;

function ChoiceExample({
  id,
  title,
  description,
  code,
  children,
}: {
  id: string;
  title: string;
  description: string;
  code: string;
  children: ReactNode;
}) {
  return (
    <Card id={id} tabIndex={-1} className="min-w-0 scroll-mt-36 md:scroll-mt-8">
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-5">
        <div className="bg-muted/20 rounded-lg border p-4">{children}</div>
        <CodeExample code={code} label={title} />
      </CardContent>
    </Card>
  );
}

function SelectedValue({ value }: { value: string | ChoiceOption | readonly ChoiceOption[] | null }) {
  const serialized = typeof value === "string" ? JSON.stringify(value) : Array.isArray(value)
    ? JSON.stringify(value.map((option: ChoiceOption) => option.value))
    : value ? JSON.stringify((value as ChoiceOption).value) : "null";

  return (
    <p className="text-muted-foreground mt-3 break-words text-xs" aria-live="polite">
      Selected value: <code className="text-foreground font-mono">{serialized}</code>
    </p>
  );
}

function AsyncHint() {
  return (
    <p className="text-muted-foreground mt-2 text-xs leading-5">
      Simulated server · 650 ms. Try <code>react</code>, <code>no-match</code>, or <code>error</code>.
      The first “error” request fails; Retry returns the list. Reload to reset.
    </p>
  );
}

function SelectExample() {
  const [value, setValue] = useState("");
  return (
    <ChoiceExample
      id="select-example"
      title="Select"
      description="Choose one value from a fixed list. A native select with familiar keyboard and mobile behavior."
      code={selectCode}
    >
      <label htmlFor="select-framework" className="mb-2 block text-sm font-medium">Framework</label>
      <Select id="select-framework" name="framework" options={frameworks} value={value} onValueChange={setValue} placeholder="Choose a framework" />
      <SelectedValue value={value} />
    </ChoiceExample>
  );
}

function MultiSelectExample() {
  const [value, setValue] = useState<ChoiceOption[]>([frameworks[0]]);
  return (
    <ChoiceExample
      id="multi-select-example"
      title="Multi select"
      description="Search a local list and select several options. Remove a tag individually or clear the selection."
      code={multiSelectCode}
    >
      <label htmlFor="multi-frameworks" className="mb-2 block text-sm font-medium">Frameworks</label>
      <MultiSelect id="multi-frameworks" name="frameworks" options={frameworks} value={value} onValueChange={setValue} placeholder="Add frameworks" />
      <SelectedValue value={value} />
    </ChoiceExample>
  );
}

function LocalAutocompleteExample() {
  const [value, setValue] = useState<ChoiceOption | null>(null);
  return (
    <ChoiceExample
      id="autocomplete-example"
      title="Autocomplete · local options"
      description="Filter supplied options as you type. Search labels or extra keywords: “jsx” finds React."
      code={localAutocompleteCode}
    >
      <label htmlFor="autocomplete-local" className="mb-2 block text-sm font-medium">Framework</label>
      <Autocomplete id="autocomplete-local" options={frameworks} value={value} onValueChange={setValue} placeholder="Search frameworks" />
      <p className="text-muted-foreground mt-2 text-xs">Try “jsx” for a keyword match or “no-match” for the empty state.</p>
      <SelectedValue value={value} />
    </ChoiceExample>
  );
}

function AsyncAutocompleteExample() {
  const [value, setValue] = useState<ChoiceOption | null>(null);
  return (
    <ChoiceExample
      id="async-autocomplete-example"
      title="Autocomplete · async options"
      description="Load results on demand. Debounced queries support loading, empty, and retry states."
      code={asyncAutocompleteCode}
    >
      <label htmlFor="autocomplete-async" className="mb-2 block text-sm font-medium">Remote framework</label>
      <Autocomplete id="autocomplete-async" loadOptions={remoteAutocomplete.loadOptions} debounceMs={250} value={value} onValueChange={setValue} placeholder="Search remote frameworks" />
      <AsyncHint />
      <SelectedValue value={value} />
    </ChoiceExample>
  );
}

function CreatableAutocompleteExample() {
  const [options, setOptions] = useState<ChoiceOption[]>(frameworks);
  const [value, setValue] = useState<ChoiceOption | null>(null);
  const [allowCreation, setAllowCreation] = useState(true);

  function onCreate(input: string) {
    const option = newOption(input);
    setOptions((current) => [...current, option]);
    return option;
  }

  return (
    <ChoiceExample
      id="creatable-autocomplete-example"
      title="Autocomplete · create an option"
      description="Offer creation when a search has no results. Toggle it below; created options stay in this local list."
      code={creatableAutocompleteCode}
    >
      <label className="mb-4 flex cursor-pointer items-center gap-2 text-sm">
        <input id="allow-creation" type="checkbox" checked={allowCreation} onChange={(event) => setAllowCreation(event.target.checked)} className="accent-primary size-4 focus-visible:outline-ring focus-visible:outline-2 focus-visible:outline-offset-2" />
        Allow creating new options
      </label>
      <label htmlFor="autocomplete-create" className="mb-2 block text-sm font-medium">Framework</label>
      <Autocomplete id="autocomplete-create" options={options} value={value} onValueChange={setValue} creatable={allowCreation} onCreate={onCreate} placeholder="Search or create a framework" />
      <p className="text-muted-foreground mt-2 text-xs">Try “Astro”, then choose Create. Clear and search again to find the saved option.</p>
      <SelectedValue value={value} />
    </ChoiceExample>
  );
}

function AsyncCreatableExample() {
  const [value, setValue] = useState<ChoiceOption | null>(null);
  return (
    <ChoiceExample
      id="async-creatable-example"
      title="Autocomplete · async + create"
      description="Combine remote search with asynchronous creation. Creation waits for a successful empty result."
      code={asyncCreatableCode}
    >
      <label htmlFor="autocomplete-async-create" className="mb-2 block text-sm font-medium">Remote framework</label>
      <Autocomplete id="autocomplete-async-create" loadOptions={remoteCreatable.loadOptions} value={value} onValueChange={setValue} creatable onCreate={remoteCreatable.onCreate} placeholder="Search or create remotely" />
      <AsyncHint />
      <p className="text-muted-foreground mt-1 text-xs">Try “Astro” to create. Saving takes 450 ms; the result remains searchable.</p>
      <SelectedValue value={value} />
    </ChoiceExample>
  );
}

function AsyncMultiSelectExample() {
  const [value, setValue] = useState<ChoiceOption[]>([]);
  return (
    <ChoiceExample
      id="async-multi-select-example"
      title="Multi select · async + create"
      description="The same async loader and optional creation work with multiple selections."
      code={asyncMultiSelectCode}
    >
      <label htmlFor="multi-async" className="mb-2 block text-sm font-medium">Remote frameworks</label>
      <MultiSelect id="multi-async" name="frameworks" loadOptions={remoteMultiSelect.loadOptions} value={value} onValueChange={setValue} creatable onCreate={remoteMultiSelect.onCreate} placeholder="Search or create frameworks" />
      <AsyncHint />
      <SelectedValue value={value} />
    </ChoiceExample>
  );
}

export function ChoiceExamples() {
  return (
    <section aria-labelledby="choice-components-title" className="flex min-w-0 flex-col gap-6">
      <div>
        <h2 id="choice-components-title" className="text-xl font-semibold tracking-tight">Selection &amp; autocomplete</h2>
        <p className="text-muted-foreground mt-2 text-sm leading-6">
          Start with a fixed list, add search, or load options asynchronously. Creation is opt-in.
          Each example shows the component props and the state needed to use it.
        </p>
        <p className="text-muted-foreground mt-4 text-xs leading-5">
          Keyboard: use ↑ / ↓ to move through search results, Enter to select, and Escape to close.
          Tab moves between controls. Angular is disabled in these examples.
        </p>
      </div>
      <SelectExample />
      <MultiSelectExample />
      <LocalAutocompleteExample />
      <AsyncAutocompleteExample />
      <CreatableAutocompleteExample />
      <AsyncCreatableExample />
      <AsyncMultiSelectExample />
    </section>
  );
}
