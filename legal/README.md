# `legal/` — documents generated from a jurisdiction dataset

The territory restrictions live here as **data**; the documents are **generated** from it. Sanctions
programmes change, and hand-editing the same prose in three places is how a legal document quietly
starts saying something untrue.

```bash
bun run legal/generate.ts            # regenerate; writes only what actually changed
bun run legal/generate.ts --check    # fail if the committed output is stale (for CI)
bun test legal                       # the validation rules and the document's safety properties
```

## Read this before you touch anything here

**The generator is a template filler, not a lawyer.** It must never invent an obligation. That rests
on one split, and everything in this directory exists to keep it:

- **The binding prose is written once, reviewed by a lawyer, and static.** It lives in
  `src/documents/<doc>.ts`. It references the *authority* — "jurisdictions subject to comprehensive
  sanctions administered by OFAC, and any jurisdiction where provision of the software is otherwise
  prohibited" — so it stays correct even when the data file is stale. It names no country.
- **The enumerated country list is informational**, generated from `src/jurisdictions.ts`, and
  labelled as such with the date it was reviewed. It is a courtesy to the reader, not the operative
  clause. Where the two disagree, the binding clause governs, and the document says so.

If the generated list ever becomes the operative clause, a stale data file makes the terms lie.

**The output is committed, never generated at read time.** A user agrees to a fixed artifact; a
document that changes underneath them is not a document. Regenerating produces a diff for a human to
review and commit — so nothing here reads the clock, and a second run over an unchanged dataset
writes nothing.

**Everything here still needs a lawyer.** `legal/terms-of-use.gen.md` is a draft. The dataset is a
record of which instruments were read and on what date; it is not a legal determination, and the
sections marked `TODO(lawyer)` are deliberately empty rather than filled with plausible-sounding
prose.

## What is where

| Path | What it is |
|---|---|
| `src/jurisdictions.ts` | The dataset. One entry per jurisdiction, each citing its own authority. |
| `src/model.ts` | The types, and the validation that stops a bad entry from reaching a document. |
| `src/sections.ts` | The parts of a document derived from the dataset — shared between documents. |
| `src/documents/terms-of-use.ts` | The Terms of Use: its static binding prose, and its assembly. |
| `generate.ts` | Validates, renders every document, writes the ones that changed. |
| `terms-of-use.gen.md` | **Generated.** Committed. Do not edit by hand — the next run overwrites it. |

## Adding or changing a jurisdiction

Edit `src/jurisdictions.ts`, then run the generator and read the diff.

An entry looks like this, and every field below is required:

```ts
{
  id: "ru",                         // lower-case, hyphenated; usually the ISO 3166-1 alpha-2 code
  name: "Russia",
  tier: "category-prohibited",      // "embargo" | "category-prohibited" | "screen-only"
  scope: "national",                // "regional" if it carries carve-outs; then `regions` is required
  authority: [                      // at least one, each with a citation you have actually read
    {
      body: "BIS",
      instrument: "15 CFR 746.8(a)(8) — Sanctions against Russia and Belarus",
      url: "https://www.ecfr.gov/current/title-15/section-746.8",
      effective: "2024-09-12",      // optional, where the date is known and load-bearing
      note: "Names project management software, and explicitly its updates.",
    },
  ],
  reviewed: "2026-08-22",           // the date you checked this entry against those instruments
  status: "open-question",          // "settled" | "open-question"
  todo: "TODO(lawyer): …",          // required when the status is open-question
}
```

The three tiers:

- **`embargo`** — comprehensive, no dealings.
- **`category-prohibited`** — the country is not embargoed, but our *product category* is
  implicated. Filing a jurisdiction here is not a finding that ADI falls inside that category; that
  is an export-classification question, and while it is open the entry says so.
- **`screen-only`** — the default everywhere else. Exactly one entry carries `isDefault: true`, and
  it is where the document says that SDN screening is country-independent: a designated person may
  not be dealt with even in a fully permitted jurisdiction.

Rules the validator enforces, and why:

- **Every entry cites at least one authority, and carries a review date.** A line without a citation
  is neither maintainable nor defensible.
- **An `open-question` entry must carry a `todo` starting with a marker** (`TODO(lawyer)`), because
  the document generates the open-questions list from those todos — an entry cannot be marked open in
  the data and quietly omitted from the document.
- **A `screen-only` entry with no regional carve-out is rejected.** That is the default, and listing
  a country by name implies a restriction the dataset does not carry. Ukraine is the reason the
  carve-out mechanism exists: no national restriction, and a comprehensive one over the occupied
  territories. Excluding the country wholesale would be both wrong and insulting; omitting the
  regional carve-out would be a compliance failure.
- **A region's tier must differ from its country's.** One that matches is not a carve-out.

The run fails and writes nothing if any of these is violated. A thinner document is worse than no
document, because it still reads as complete.

## The telemetry statement

`src/jurisdictions.ts` also carries the telemetry facts, with the date they were last checked and
how. **Re-verify them against the source tree before you move that date** — they are generated into
a legal document as statements of fact. They live in the dataset rather than in one document because
the privacy policy will need the same sentences.

## Adding a second document

A document is a module exporting `{ path, render(data) }` and a line in `DOCUMENTS` in
`generate.ts`. Put the data-derived parts in `src/sections.ts` so both documents share them, and the
binding prose in the document's own module. `render` must be deterministic and must never read the
clock.
