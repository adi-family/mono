/**
 * Generates the legal documents in this directory from `legal/src/jurisdictions.ts`.
 *
 *     bun run legal/generate.ts            # write the documents
 *     bun run legal/generate.ts --check    # fail if what is committed is not what this produces
 *
 * **This is a template filler, not a lawyer, and it must never invent an obligation.** The split it
 * exists to keep is:
 *
 * * the binding prose is written once, reviewed once, and static. It references the *authority* —
 *   "jurisdictions subject to comprehensive sanctions administered by OFAC, and any jurisdiction
 *   where provision of the software is otherwise prohibited" — so it stays correct even when the
 *   dataset is stale;
 * * the enumerated country list is **informational**, generated, and labelled as such with the date
 *   it was reviewed.
 *
 * If the generated list ever becomes the operative clause, a stale data file makes the terms lie.
 * That is the failure this design is built to prevent, and it is the reason for every rule in
 * `src/model.ts` that stops a run rather than producing a thinner document.
 *
 * The output is committed, never generated at read time: a user agrees to a fixed artifact, and a
 * document that changes underneath them is not a document. Regenerating produces a diff for a human
 * to read and commit — so nothing here reads the clock, and a second run over an unchanged dataset
 * writes nothing at all.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { TERMS_OF_USE } from "./src/documents/terms-of-use";
import { DATASET } from "./src/jurisdictions";
import { validate, type Document } from "./src/model";

/** Every document generated from the dataset. A second one is a module and a line here. */
const DOCUMENTS: Document[] = [TERMS_OF_USE];

const ROOT = join(import.meta.dir, "..");

function main(): number {
  const check = process.argv.includes("--check");

  const problems = validate(DATASET);
  if (problems.length > 0) {
    console.error(`legal: the dataset has ${problems.length} problem(s); nothing was written.\n`);
    for (const p of problems) console.error(`  ${p}`);
    return 1;
  }

  let changed = 0;
  for (const doc of DOCUMENTS) {
    const target = join(ROOT, doc.path);
    const next = doc.render(DATASET);
    const current = readIfPresent(target);

    if (current === next) {
      console.log(`  unchanged  ${doc.path}`);
      continue;
    }

    changed += 1;
    if (check) {
      console.error(`  STALE      ${doc.path}`);
      continue;
    }

    writeFileSync(target, next, "utf8");
    console.log(`  ${current === null ? "created" : "updated"}    ${doc.path}`);
  }

  if (check && changed > 0) {
    console.error(
      `\nlegal: ${changed} document(s) differ from the dataset. Run \`bun run legal/generate.ts\`` +
        ` and commit the diff.`,
    );
    return 1;
  }
  return 0;
}

function readIfPresent(path: string): string | null {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return null;
  }
}

process.exit(main());
