/**
 * The shape of the jurisdiction dataset, and the validation every entry must pass before it can
 * reach a generated document.
 *
 * {@link validate} fails the run rather than degrading it. A legal document that silently dropped
 * a jurisdiction — because its citation was missing, or its tier was misspelt — would still read
 * as complete, and nobody reviewing the diff would see the absence. Every rule below exists to
 * turn a thinner document into a stopped build.
 */

/**
 * How restricted a jurisdiction is. The three tiers are the ones set out in
 * `business/memos/2026-08-22-territories-and-the-two-commitments.md`; they are not legal terms of
 * art, they are how this project files a jurisdiction.
 */
export type Tier = "embargo" | "category-prohibited" | "screen-only";

/**
 * Whether the entry's tier is the whole story for that jurisdiction.
 *
 * `"national"` — the tier applies country-wide and nothing narrower is carved out.
 * `"regional"` — the tier applies country-wide *and* the entry carries {@link Region}s that are
 * treated differently within their own area. Ukraine is the case this exists for: no national
 * restriction, and a comprehensive one over the occupied territories.
 */
export type Scope = "national" | "regional";

/**
 * Whether we consider the entry resolved. `"open-question"` means a real question is outstanding
 * with a lawyer — it is not a placeholder for "not looked at yet", and it forces a {@link
 * Jurisdiction.todo} that the generator surfaces in the document.
 */
export type Status = "settled" | "open-question";

/** One instrument that a restriction rests on. No entry may exist without at least one. */
export interface Authority {
  /** The body that issued it — `"OFAC"`, `"BIS"`, `"EU"`. */
  body: string;

  /** Cited the way it is normally cited: `"15 CFR 746.8(a)(8)"`, `"31 CFR Part 515"`. */
  instrument: string;

  /**
   * A link that will still resolve in a year — a CFR part on eCFR, a programme page, an ELI
   * reference. Prefer the stable parent over a deep link into a PDF that gets reissued.
   */
  url: string;

  /** ISO date the instrument took effect, where that date is known and load-bearing. */
  effective?: string;

  /** What this instrument contributes that its title does not already say. */
  note?: string;
}

/** A restriction that applies to part of a jurisdiction rather than all of it. */
export interface Region {
  /** Stable id, unique within its jurisdiction. */
  id: string;

  /** As it should be printed. */
  name: string;

  /** Must differ from the parent's tier — a region that matches it is not a carve-out. */
  tier: Tier;

  /** At least one. */
  authority: Authority[];

  status: Status;

  /** Required when `status` is `"open-question"`. Printed in the document's open-questions list. */
  todo?: string;

  note?: string;
}

/** One jurisdiction, with everything needed to print it and to defend printing it. */
export interface Jurisdiction {
  /** Stable id — lower-case, hyphenated. Usually the ISO 3166-1 alpha-2 code. */
  id: string;

  /** As it should be printed. */
  name: string;

  tier: Tier;

  scope: Scope;

  /** Present exactly when `scope` is `"regional"`. */
  regions?: Region[];

  /** At least one. A jurisdiction without a citation is neither maintainable nor defensible. */
  authority: Authority[];

  /** ISO date this entry was last checked against its authorities. */
  reviewed: string;

  status: Status;

  /** Required when `status` is `"open-question"`. Printed in the document's open-questions list. */
  todo?: string;

  note?: string;

  /**
   * The catch-all entry for every jurisdiction not named in the dataset. Exactly one entry carries
   * it, and it is never printed as a country — it is where the document says what applies by
   * default.
   */
  isDefault?: boolean;
}

/**
 * A statement of fact about the product, carrying the date it was last checked against the source
 * tree. Facts age; a statement generated into a legal document without the date it was verified is
 * how a document starts saying something that used to be true.
 */
export interface FactualStatement {
  /** ISO date the facts below were last confirmed. */
  verified: string;

  /** How they were confirmed, in a few words. Printed, so a reader can judge the claim. */
  method: string;

  /** One sentence each. These are printed verbatim. */
  facts: string[];
}

/** Everything the documents are generated from. */
export interface Dataset {
  jurisdictions: Jurisdiction[];
  telemetry: FactualStatement;
}

/** A document this directory generates. Adding one is adding a module that exports one of these. */
export interface Document {
  /** Where it is written, relative to the repository root. */
  path: string;

  /**
   * The whole file, as text. Must be deterministic: the same dataset in, the same bytes out. In
   * particular it must never read the clock — a document that re-dates itself on every run makes
   * every regeneration a diff, which trains a reviewer to skim the one that matters.
   */
  render(data: Dataset): string;
}

/** How each tier is described wherever a document has to name it. */
export const TIERS: Record<Tier, { label: string; effect: string; order: number }> = {
  embargo: {
    label: "Comprehensive embargo",
    effect: "No dealings of any kind.",
    order: 0,
  },
  "category-prohibited": {
    label: "Product category prohibited",
    effect: "Not embargoed; the restriction attaches to software of this kind.",
    order: 1,
  },
  "screen-only": {
    label: "No territorial restriction",
    effect: "Available, subject to the person-level screening that applies everywhere.",
    order: 2,
  },
};

const ID = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/;

function isIsoDate(value: string): boolean {
  if (!ISO_DATE.test(value)) return false;
  const parsed = new Date(`${value}T00:00:00Z`);
  // Round-trips only for a date that exists: `2026-02-30` parses, then prints as March 2nd.
  return !Number.isNaN(parsed.getTime()) && parsed.toISOString().slice(0, 10) === value;
}

/**
 * Every problem with the dataset, as human-readable lines. Empty means the dataset is fit to
 * generate from.
 *
 * It collects rather than throwing on the first fault so that one run tells you everything that
 * needs fixing.
 */
export function validate(data: Dataset): string[] {
  const problems: string[] = [];
  const at = (where: string, what: string) => problems.push(`${where}: ${what}`);

  if (data.jurisdictions.length === 0) problems.push("dataset: no jurisdictions");

  const seen = new Set<string>();
  for (const j of data.jurisdictions) {
    const where = `jurisdiction ${j.id || "<no id>"}`;

    if (!ID.test(j.id ?? "")) at(where, "id must be lower-case and hyphenated");
    if (seen.has(j.id)) at(where, "duplicate id");
    seen.add(j.id);

    if (!j.name?.trim()) at(where, "missing name");
    if (!(j.tier in TIERS)) at(where, `unknown tier ${JSON.stringify(j.tier)}`);
    if (j.scope !== "national" && j.scope !== "regional") {
      at(where, `unknown scope ${JSON.stringify(j.scope)}`);
    }

    const regions = j.regions ?? [];
    if (j.scope === "regional" && regions.length === 0) {
      at(where, "scope is regional but no regions are listed");
    }
    if (j.scope === "national" && regions.length > 0) {
      at(where, "scope is national but regions are listed");
    }

    problems.push(...checkAuthorities(where, j.authority));
    if (!isIsoDate(j.reviewed ?? "")) at(where, "reviewed must be an ISO date (YYYY-MM-DD)");
    problems.push(...checkStatus(where, j.status, j.todo));

    // A screen-only entry says "nothing here is restricted", which is what the default entry
    // already says for every country in the world. Listing one by name implies a restriction the
    // dataset does not carry — unless it exists to hang a regional carve-out off.
    if (!j.isDefault && j.tier === "screen-only" && regions.length === 0) {
      at(where, "screen-only with no regional carve-out — this is the default, so do not list it");
    }

    const seenRegions = new Set<string>();
    for (const r of regions) {
      const rWhere = `${where} / region ${r.id || "<no id>"}`;
      if (!ID.test(r.id ?? "")) at(rWhere, "id must be lower-case and hyphenated");
      if (seenRegions.has(r.id)) at(rWhere, "duplicate id");
      seenRegions.add(r.id);

      if (!r.name?.trim()) at(rWhere, "missing name");
      if (!(r.tier in TIERS)) at(rWhere, `unknown tier ${JSON.stringify(r.tier)}`);
      if (r.tier === j.tier) at(rWhere, "region tier equals the national tier — not a carve-out");

      problems.push(...checkAuthorities(rWhere, r.authority));
      problems.push(...checkStatus(rWhere, r.status, r.todo));
    }
  }

  const defaults = data.jurisdictions.filter((j) => j.isDefault);
  if (defaults.length !== 1) {
    problems.push(`dataset: expected exactly one default entry, found ${defaults.length}`);
  }
  for (const d of defaults) {
    if (d.tier !== "screen-only" || d.scope !== "national") {
      at(`jurisdiction ${d.id}`, "the default entry must be screen-only and national");
    }
  }

  const t = data.telemetry;
  if (!isIsoDate(t?.verified ?? "")) {
    problems.push("telemetry: verified must be an ISO date (YYYY-MM-DD)");
  }
  if (!t?.method?.trim()) problems.push("telemetry: missing method");
  if (!t?.facts?.length) problems.push("telemetry: no facts");

  return problems;
}

function checkAuthorities(where: string, authority: Authority[] | undefined): string[] {
  if (!authority?.length) return [`${where}: no authority cited`];

  const problems: string[] = [];
  authority.forEach((a, i) => {
    const aWhere = `${where} / authority[${i}]`;
    if (!a.body?.trim()) problems.push(`${aWhere}: missing body`);
    if (!a.instrument?.trim()) problems.push(`${aWhere}: missing instrument`);
    if (!a.url?.startsWith("https://")) problems.push(`${aWhere}: url must be https`);
    if (a.effective !== undefined && !isIsoDate(a.effective)) {
      problems.push(`${aWhere}: effective must be an ISO date (YYYY-MM-DD)`);
    }
  });
  return problems;
}

function checkStatus(where: string, status: Status, todo: string | undefined): string[] {
  if (status !== "settled" && status !== "open-question") {
    return [`${where}: unknown status ${JSON.stringify(status)}`];
  }
  if (status === "open-question" && !todo?.trim()) {
    return [`${where}: open-question needs a todo saying what is open and who resolves it`];
  }
  // The marker is in the data, not added by the renderer, so that grepping the tree for
  // `TODO(lawyer)` finds the dataset as well as the prose.
  if (status === "open-question" && !todo!.startsWith("TODO(")) {
    return [`${where}: todo must start with a marker naming who resolves it, e.g. TODO(lawyer)`];
  }
  if (status === "settled" && todo?.trim()) {
    return [`${where}: settled entries must not carry a todo`];
  }
  return [];
}

/** Tier first, then name — so a regeneration diff shows a real change and never a reordering. */
export function sorted(jurisdictions: Jurisdiction[]): Jurisdiction[] {
  return [...jurisdictions].sort(
    (a, b) => TIERS[a.tier].order - TIERS[b.tier].order || a.name.localeCompare(b.name, "en"),
  );
}

/**
 * The date the list as a whole can honestly claim.
 *
 * It is the *oldest* review date in the dataset, not the newest: "as of" is a promise that every
 * line was checked by then, and one entry reviewed this morning says nothing about the entry
 * nobody has looked at since last year. Taking the newest would let a one-line edit re-date the
 * whole document.
 */
export function reviewedThrough(jurisdictions: Jurisdiction[]): string {
  return jurisdictions.map((j) => j.reviewed).sort()[0]!;
}
