import { useState } from "react";
import { createRoot } from "react-dom/client";

import { Autocomplete, MultiSelect, Select, type ChoiceOption } from "../src/components";

const options: ChoiceOption[] = [
  { value: "alpha", label: "Alpha" },
  { value: "beta", label: "Beta", disabled: true },
  { value: "gamma", label: "Gamma", keywords: ["third"] },
];

const state = {
  requests: [] as string[],
  aborted: [] as string[],
  retries: 0,
  created: [] as string[],
};

async function loadOptions(query: string, { signal }: { signal: AbortSignal }) {
  state.requests.push(query);
  signal.addEventListener("abort", () => state.aborted.push(query), { once: true });
  // Deliberately finish even after cancellation: components must ignore stale results.
  await new Promise((resolve) => setTimeout(resolve, query === "old" ? 350 : 70));
  if (query === "error") throw new Error("Network unavailable");
  if (query === "retry" && state.retries++ === 0) throw new Error("Please retry");
  if (query === "empty") return [];
  if (query === "old" || query === "new" || query === "retry") {
    return [{ value: query, label: `${query.toUpperCase()} result` }];
  }
  return options;
}

async function createOption(label: string) {
  state.created.push(label);
  await new Promise((resolve) => setTimeout(resolve, label === "Pending" ? 700 : 100));
  if (label === "reject") throw new Error("Creation refused");
  return { value: `created-${label.toLowerCase()}`, label };
}

function Fixture() {
  const [native, setNative] = useState("");
  const [single, setSingle] = useState<ChoiceOption | null>(null);
  const [multi, setMulti] = useState<ChoiceOption[]>([]);
  const [created, setCreated] = useState<ChoiceOption | null>(null);
  const [refreshed, setRefreshed] = useState(false);

  return (
    <main style={{ maxWidth: 520, padding: 24, display: "grid", gap: 16 }}>
      <h1>Choice control browser fixtures</h1>
      <label htmlFor="test-select">Native select</label>
      <Select id="test-select" options={options} placeholder="Choose" value={native} onValueChange={setNative} />
      <output id="native-value">{native}</output>
      <label htmlFor="test-local">Local autocomplete</label>
      <Autocomplete id="test-local" options={options} value={single} onValueChange={setSingle} />
      <output id="single-value">{JSON.stringify(single)}</output>
      <label htmlFor="test-multi">Multiple selection</label>
      <MultiSelect id="test-multi" options={options} value={multi} onValueChange={setMulti} />
      <output id="multi-value">{JSON.stringify(multi)}</output>
      <label htmlFor="test-async">Remote autocomplete</label>
      <Autocomplete id="test-async" loadOptions={loadOptions} debounceMs={20} />
      <label htmlFor="test-async-multi">Remote multiple selection</label>
      <MultiSelect id="test-async-multi" loadOptions={loadOptions} debounceMs={20} />
      <label htmlFor="test-create">Create an option</label>
      <Autocomplete id="test-create" options={options} creatable onCreate={createOption} value={created} onValueChange={setCreated} />
      <output id="created-value">{JSON.stringify(created)}</output>
      <label htmlFor="test-async-create">Remote and creatable</label>
      <Autocomplete id="test-async-create" loadOptions={loadOptions} debounceMs={20} creatable onCreate={createOption} />
      <label htmlFor="test-default-create">Create without a handler</label>
      <MultiSelect id="test-default-create" options={options} creatable />
      <MultiSelect id="test-locked" aria-label="Locked selection" options={options} defaultValue={[options[0], options[1]]} />
      <MultiSelect id="test-refresh" aria-label="Refreshed options" options={refreshed ? [{ value: "Fresh", label: "Updated", disabled: true }] : []} creatable />
      <button id="test-refresh-options" type="button" onClick={() => setRefreshed(true)}>Refresh options</button>
      <form id="test-required-form">
        <Autocomplete id="test-required" aria-label="Required choice" required creatable onCreate={createOption} />
      </form>
      <Select id="test-disabled-select" aria-label="Disabled select" options={options} disabled />
      <Autocomplete id="test-disabled-local" aria-label="Disabled autocomplete" options={options} disabled />
      <MultiSelect id="test-disabled-multi" aria-label="Disabled multi select" options={options} disabled />
      <form id="test-form">
        <Select id="test-form-select" aria-label="Form select" name="native" options={options} defaultValue="alpha" />
        <Autocomplete id="test-form-single" aria-label="Form autocomplete" name="single" options={options} defaultValue={options[0]} />
        <MultiSelect id="test-form-multi" aria-label="Form multi select" name="many" options={options} defaultValue={[options[0]]} />
        <button type="reset" id="test-reset">Reset</button>
      </form>
      <button id="test-outside" type="button">Outside the controls</button>
    </main>
  );
}

export function mount() {
  const gallery = document.getElementById("root");
  if (gallery) gallery.style.display = "none";
  const host = document.createElement("div");
  host.id = "choice-browser-fixture";
  document.body.appendChild(host);
  createRoot(host).render(<Fixture />);
  Object.assign(window, { __choiceTest: state });
}
