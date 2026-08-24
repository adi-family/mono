/**
 * The Terms of Use document.
 *
 * Everything binding in it is the static prose in this file: written once, meant to be reviewed by
 * a lawyer once, and phrased to reference the *authority* rather than a list of countries, so that
 * it stays correct while the dataset ages. Everything generated from
 * `legal/src/jurisdictions.ts` is informational — a dated courtesy summary — and the document says
 * so where a reader will see it, in the section that carries the list.
 *
 * The rule for anyone editing this file: **the generator does not write law.** A sentence that
 * creates an obligation belongs here, in reviewed static prose. If the memo this is drawn from is
 * silent on a point, the document leaves a `TODO(lawyer)` rather than a plausible-sounding
 * paragraph — a generated obligation nobody wrote is exactly the failure this design exists to
 * prevent.
 */

import { blocks, generatedBanner, quote } from "../md";
import { reviewedThrough, type Dataset, type Document } from "../model";
import {
  everywhereElse,
  openQuestions,
  telemetryFacts,
  territoryDetail,
  territoryTable,
} from "../sections";

const COMMAND = "bun run legal/generate.ts";

const SOURCES = ["`legal/src/jurisdictions.ts`", "`legal/src/documents/terms-of-use.ts`"];

const DRAFT_NOTICE = `**Draft. Not reviewed by a lawyer, and not in force.**
It is assembled from a working list of jurisdictions kept in this repository, which is a record of
which instruments were read and when — not a legal determination. Sections marked \`TODO(lawyer)\`
are deliberately empty rather than filled with something that reads like law.`;

const HOW_TO_READ = `This document contains two kinds of text, and they do not carry the same weight.

**Binding sections** are marked *binding*. They are written prose. They name no country, on
purpose: sanctions programmes change, and a clause that enumerates jurisdictions goes stale and
then states something untrue, while a clause that references the authority stays correct without
maintenance.

**Informational sections** are marked *informational*. They are generated from a dataset, they
carry the date that dataset was last reviewed, and they are a courtesy to the reader. They are not
operative. Where an informational section and a binding section disagree — because the list has
aged, or because an entry was misfiled — **the binding section governs.**`;

const TERRITORY_CLAUSE = `ADI is not available in any jurisdiction or region subject to comprehensive sanctions administered
by the U.S. Department of the Treasury's Office of Foreign Assets Control (OFAC), or in any
jurisdiction where provision of the software is otherwise prohibited.

This clause names no jurisdiction. It is written against the sanctions authority itself, so that it
remains accurate whether or not the list in the next section has kept up with it.`;

const PERSON_CLAUSE = `Sanctions restrictions attach to persons as well as to places. A person or entity designated on
OFAC's Specially Designated Nationals and Blocked Persons (SDN) List may not be dealt with in any
jurisdiction — including one where ADI is otherwise fully available and carries no territorial
restriction at all.

Counterparties are screened against that list before an enterprise sale.`;

const TELEMETRY_INTRO = `This section states facts, not intentions. Each line was checked against the source tree on the
date given below. Where a line speaks about something ADI does not currently do, it is an
undertaking about how such a thing would be introduced if it ever were.`;

const NOT_WRITTEN = `These are the sections a complete Terms of Use needs and this draft does not have. They are listed
rather than drafted because the working memo behind this document is silent on them, and prose that
sounds like law but was written by nobody is worse than a visible gap.

- \`TODO(lawyer)\`: acceptance and formation — what act constitutes agreement to these terms.
- \`TODO(lawyer)\`: the relationship between these terms and \`LICENSE\`, and which governs on a
  conflict. The licence is under separate revision and is deliberately untouched here.
- \`TODO(lawyer)\`: whether a user must represent that they are not located in a restricted
  jurisdiction, and at which point that representation attaches — download, payment, or support.
- \`TODO(lawyer)\`: warranty disclaimer.
- \`TODO(lawyer)\`: limitation of liability.
- \`TODO(lawyer)\`: governing law and venue.
- \`TODO(lawyer)\`: how these terms may change, and what notice is given.
- \`TODO(lawyer)\`: contact and notices.
- \`TODO(lawyer)\`: confirm the precedence wording in *How to read this document* — that the
  generated territory list is informational and the binding clause governs on a conflict.`;

function render(data: Dataset): string {
  const asOf = reviewedThrough(data.jurisdictions);

  return `${blocks(
    generatedBanner(COMMAND, SOURCES),
    "# ADI — Terms of Use",
    quote(DRAFT_NOTICE),
    "## How to read this document",
    HOW_TO_READ,
    "## 1. Where ADI is available — *binding*",
    TERRITORY_CLAUSE,
    `## 2. Territories as of ${asOf} — *informational*`,
    `The list below is a courtesy summary of where the jurisdictions we have looked at stood when this
dataset was last reviewed. Every entry was checked on or after **${asOf}**; each carries its own
review date and the instruments it rests on, so a reader can check it rather than take it on trust.
It is not part of section 1, and section 1 governs where the two differ.`,
    territoryTable(data),
    territoryDetail(data),
    everywhereElse(data),
    "## 3. People, not only places — *binding*",
    PERSON_CLAUSE,
    "## 4. Telemetry and where your data lives — *statement of fact*",
    TELEMETRY_INTRO,
    telemetryFacts(data),
    "## 5. Open questions carried by the territory list — *informational*",
    `Filing a jurisdiction under a tier is not a determination that ADI is prohibited there. Where a
question is genuinely open, the dataset says so and it is reproduced here rather than resolved
quietly in the reader's favour or ours.`,
    openQuestions(data),
    "## 6. Not written yet",
    NOT_WRITTEN,
  )}\n`;
}

export const TERMS_OF_USE: Document = {
  path: "legal/terms-of-use.gen.md",
  render,
};
