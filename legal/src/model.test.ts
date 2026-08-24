/**
 * The rules that stop a bad dataset from becoming a document.
 *
 * Each case here corresponds to a way a legal document could go quietly wrong: a jurisdiction with
 * no citation behind it, a review date nobody can act on, an open question that never reaches the
 * reader, or a country listed as restricted when the dataset does not say it is.
 *
 *     bun test legal
 */

import { describe, expect, test } from "bun:test";

import { TERMS_OF_USE } from "./documents/terms-of-use";
import { DATASET } from "./jurisdictions";
import { validate, type Dataset } from "./model";

/** The real dataset, deep-copied so a case can break one field without affecting the others. */
function broken(mutate: (d: Dataset) => void): string[] {
  const copy = structuredClone(DATASET) as Dataset;
  mutate(copy);
  return validate(copy);
}

const ua = (d: Dataset) => d.jurisdictions.find((j) => j.id === "ua")!;
const cu = (d: Dataset) => d.jurisdictions.find((j) => j.id === "cu")!;

describe("validate", () => {
  test("the committed dataset is clean", () => {
    expect(validate(DATASET)).toEqual([]);
  });

  test("a jurisdiction with no authority is rejected", () => {
    expect(broken((d) => (cu(d).authority = []))).toEqual(["jurisdiction cu: no authority cited"]);
  });

  test("an authority needs a body, an instrument and an https url", () => {
    const problems = broken((d) => (cu(d).authority[0] = { body: "", instrument: "", url: "x" }));
    expect(problems).toHaveLength(3);
  });

  test("a review date must be a real ISO date", () => {
    expect(broken((d) => (cu(d).reviewed = "2026-02-30"))).toContain(
      "jurisdiction cu: reviewed must be an ISO date (YYYY-MM-DD)",
    );
  });

  test("an open question must say what is open and who resolves it", () => {
    expect(broken((d) => delete d.jurisdictions.find((j) => j.id === "ru")!.todo)).toContain(
      "jurisdiction ru: open-question needs a todo saying what is open and who resolves it",
    );
  });

  test("a todo must carry a marker somebody can grep for", () => {
    expect(broken((d) => (d.jurisdictions.find((j) => j.id === "ru")!.todo = "ask someone"))).toContain(
      "jurisdiction ru: todo must start with a marker naming who resolves it, e.g. TODO(lawyer)",
    );
  });

  test("an unrestricted country is the default, not an entry", () => {
    expect(
      broken((d) => {
        const j = ua(d);
        j.scope = "national";
        delete j.regions;
      }),
    ).toContain(
      "jurisdiction ua: screen-only with no regional carve-out — this is the default, so do not list it",
    );
  });

  test("a region that matches its country's tier is not a carve-out", () => {
    expect(broken((d) => (ua(d).regions![0]!.tier = "screen-only"))).toContain(
      "jurisdiction ua / region crimea: region tier equals the national tier — not a carve-out",
    );
  });

  test("regional scope without regions is rejected", () => {
    expect(broken((d) => (ua(d).regions = []))).toContain(
      "jurisdiction ua: scope is regional but no regions are listed",
    );
  });

  test("exactly one entry may be the default", () => {
    expect(broken((d) => (cu(d).isDefault = true))).toContain(
      "dataset: expected exactly one default entry, found 2",
    );
  });

  test("the telemetry statement must carry the date it was verified", () => {
    expect(broken((d) => (d.telemetry.verified = "recently"))).toContain(
      "telemetry: verified must be an ISO date (YYYY-MM-DD)",
    );
  });

  test("every problem is reported, not just the first", () => {
    const problems = broken((d) => {
      cu(d).authority = [];
      cu(d).reviewed = "";
    });
    expect(problems.length).toBeGreaterThan(1);
  });
});

describe("terms of use", () => {
  const rendered = TERMS_OF_USE.render(DATASET);

  test("renders the same bytes twice — a regeneration diff is a real change", () => {
    expect(TERMS_OF_USE.render(DATASET)).toBe(rendered);
  });

  test("carries no clock-derived date", () => {
    const thisYear = String(new Date().getFullYear());
    const dates = [...rendered.matchAll(/\b(\d{4}-\d{2}-\d{2})\b/g)].map((m) => m[1]!);
    const known = new Set([
      ...DATASET.jurisdictions.flatMap((j) => [
        j.reviewed,
        ...j.authority.map((a) => a.effective),
        ...(j.regions ?? []).flatMap((r) => r.authority.map((a) => a.effective)),
      ]),
      DATASET.telemetry.verified,
    ]);
    // Not a tautology: the guard is that the document's dates all come from the dataset. A
    // `Date.now()` slipping in would print today, which is in no entry unless one was edited today.
    expect(dates.filter((d) => !known.has(d))).toEqual([]);
    expect(rendered.includes(`${thisYear}-`)).toBe(known.has(`${thisYear}-08-22`));
  });

  test("the binding clause names no jurisdiction", () => {
    const clause = rendered.split("## 1.")[1]!.split("## 2.")[0]!;
    for (const j of DATASET.jurisdictions) {
      if (!j.isDefault) expect(clause).not.toContain(j.name);
    }
  });

  test("the list is labelled informational and says the clause governs", () => {
    const list = rendered.split("## 2.")[1]!.split("## 3.")[0]!;
    expect(list).toContain("*informational*");
    expect(list).toContain("section 1 governs");
  });

  test("every open question in the dataset reaches the reader", () => {
    // Compared with the line wrapping collapsed: a todo is printed whole, but not on one line.
    const flat = rendered.replace(/\s+/g, " ");
    for (const j of DATASET.jurisdictions) {
      if (j.status === "open-question") expect(flat).toContain(j.todo!.replace(/\s+/g, " "));
      for (const r of j.regions ?? []) {
        if (r.status === "open-question") expect(flat).toContain(r.todo!.replace(/\s+/g, " "));
      }
    }
  });
});
