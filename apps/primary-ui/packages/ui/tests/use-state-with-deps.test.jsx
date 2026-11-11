import { describe, expect, test } from "bun:test";
import * as React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { useStateWithDeps } from "../src/components";

// Render-phase state updates make React re-run the component within one server render, which
// is enough to drive the hook through a local edit and then a dependency change without a DOM.
function Probe({ log }) {
  const [step, setStep] = React.useState(0);
  const key = step < 2 ? "a" : "b";
  const [value, setValue] = useStateWithDeps(() => {
    log.push(`read:${key}`);
    return `from-${key}`;
  }, [key]);

  if (step === 0) {
    setValue("edited");
    setStep(1);
  } else if (step === 1) {
    log.push(`step1:${value}`);
    setValue((previous) => `${previous}-twice`);
    setStep(2);
  } else {
    log.push(`step2:${value}`);
  }
  return <span>{value}</span>;
}

describe("useStateWithDeps", () => {
  test("starts from the reader, keeps local edits, and resets when a dependency changes", () => {
    const log = [];
    const markup = renderToStaticMarkup(<Probe log={log} />);

    expect(markup).toBe("<span>from-b</span>");
    expect(log.filter((entry) => entry.startsWith("read:"))).toEqual(["read:a", "read:b"]);
    expect(log.filter((entry) => entry.startsWith("step1:"))).toEqual(["step1:edited"]);
    // The reset render and the confirming re-render both run step 2; neither may show the edit.
    const afterChange = log.filter((entry) => entry.startsWith("step2:"));
    expect(afterChange.length).toBeGreaterThan(0);
    expect(new Set(afterChange)).toEqual(new Set(["step2:from-b"]));
  });

  test("does not reread when dependencies are unchanged", () => {
    let reads = 0;
    function Stable() {
      const [count, setCount] = React.useState(0);
      const [value] = useStateWithDeps(() => { reads += 1; return "once"; }, ["fixed", 1]);
      if (count < 2) setCount(count + 1);
      return <span>{value}</span>;
    }
    expect(renderToStaticMarkup(<Stable />)).toBe("<span>once</span>");
    expect(reads).toBe(1);
  });
});
