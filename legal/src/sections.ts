/**
 * The parts of a document that are derived from the dataset rather than written.
 *
 * They live apart from any one document because the privacy policy will need the same territory
 * list and the same telemetry facts, and two copies of a list of sanctioned jurisdictions is two
 * things to keep in step. Nothing in here states an obligation — these functions render facts,
 * citations and dates. The binding prose lives in the document modules, written once and static.
 */

import { blocks, bullet, table, wrap } from "./md";
import { sorted, TIERS, type Authority, type Dataset, type Jurisdiction, type Region } from "./model";

/** The entries a document lists by name: everything except the catch-all default. */
export function listed(data: Dataset): Jurisdiction[] {
  return sorted(data.jurisdictions.filter((j) => !j.isDefault));
}

/** The catch-all entry — what applies to a country the dataset does not name. */
export function fallback(data: Dataset): Jurisdiction {
  return data.jurisdictions.find((j) => j.isDefault)!;
}

/** `Comprehensive embargo` → `comprehensive embargo`, for use mid-sentence. */
function tierPhrase(tier: Jurisdiction["tier"]): string {
  const label = TIERS[tier].label;
  return label.charAt(0).toLowerCase() + label.slice(1);
}

function statusPhrase(status: Jurisdiction["status"]): string {
  return status === "open-question" ? "open question" : "settled";
}

/**
 * One line, however long the URL makes it. A wrapped citation would either break the link or become
 * a lazy continuation that Markdown folds back into the sentence before it.
 */
function authorityLine(a: Authority, indent = ""): string {
  const effective = a.effective ? `, effective ${a.effective}` : "";
  const note = a.note ? ` · ${a.note}` : "";
  return `${indent}- **${a.body}** — [${a.instrument}](${a.url})${effective}${note}`;
}

/** One row per named jurisdiction — the skimmable version of {@link territoryDetail}. */
export function territoryTable(data: Dataset): string {
  const rows = listed(data).map((j) => {
    const carveOuts = j.regions?.length
      ? ` — ${j.regions.length} regional carve-out${j.regions.length === 1 ? "" : "s"}`
      : "";
    return [j.name, `${TIERS[j.tier].label}${carveOuts}`, statusPhrase(j.status), j.reviewed];
  });
  return table(["Jurisdiction", "How we file it", "Status", "Reviewed"], rows);
}

/**
 * A list item holding a paragraph and a nested list, which Markdown only reads as one item if the
 * parts are separated by blank lines and indented under it — run them together and a note becomes
 * a continuation of the heading line above it.
 */
function regionBlock(r: Region): string {
  const head = bullet(`**${r.name}** — ${tierPhrase(r.tier)} · ${statusPhrase(r.status)}`);
  const authorities = r.authority.map((a) => authorityLine(a, "  ")).join("\n");
  return blocks(head, r.note ? wrap(r.note, "  ") : null, authorities);
}

function detailBlock(j: Jurisdiction): string {
  const heading = `### ${j.name} — ${tierPhrase(j.tier)}`;
  const meta = wrap(`*Reviewed ${j.reviewed} · ${statusPhrase(j.status)}.* ${TIERS[j.tier].effect}`);
  const authorities = blocks("Authorities:", j.authority.map((a) => authorityLine(a)).join("\n"));
  const regions = j.regions?.length
    ? blocks("Regional carve-outs:", j.regions.map(regionBlock).join("\n\n"))
    : null;
  return blocks(heading, meta, j.note ? wrap(j.note) : null, authorities, regions);
}

/** Every named jurisdiction, with the instruments it rests on and the date it was checked. */
export function territoryDetail(data: Dataset): string {
  return listed(data).map(detailBlock).join("\n\n");
}

/** What applies to every country the list does not name. */
export function everywhereElse(data: Dataset): string {
  return detailBlock(fallback(data));
}

/**
 * Every unresolved question the dataset carries, jurisdictions and regions alike.
 *
 * Generated rather than written so that an entry cannot be marked open in the data and quietly
 * omitted from the document — which is the failure mode that would let a reader take a tier for a
 * determination.
 */
export function openQuestions(data: Dataset): string {
  const lines: string[] = [];
  for (const j of sorted(data.jurisdictions)) {
    if (j.status === "open-question" && j.todo) lines.push(bullet(`**${j.name}** — ${j.todo}`));
    for (const r of j.regions ?? []) {
      if (r.status === "open-question" && r.todo) {
        lines.push(bullet(`**${j.name} / ${r.name}** — ${r.todo}`));
      }
    }
  }
  return lines.join("\n");
}

/** The telemetry facts, with the date and method they were last checked by. */
export function telemetryFacts(data: Dataset): string {
  const { verified, method, facts } = data.telemetry;
  return blocks(`Verified on ${verified} by ${method}:`, facts.map((f) => bullet(f)).join("\n"));
}
