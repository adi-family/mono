/**
 * The territory dataset every legal document in this directory is generated from.
 *
 * Sourced from `business/memos/2026-08-22-territories-and-the-two-commitments.md`, which is a
 * working list to take to a lawyer and **not a legal determination**. Nothing here is a conclusion
 * about what the law requires; it is a record of which instruments were read, on what date, and
 * what was still open when the entry was written.
 *
 * Every URL below was fetched on 2026-08-22 and resolved. CFR parts were confirmed against the
 * eCFR versioner API rather than by loading the page, because eCFR renders client-side and answers
 * `200` for a part that no longer exists.
 *
 * To add or change a jurisdiction, see `legal/README.md`.
 */

import type { Dataset, Jurisdiction } from "./model";

const OFAC_PROGRAMMES = "https://ofac.treasury.gov/sanctions-programs-and-country-information";

export const JURISDICTIONS: Jurisdiction[] = [
  {
    id: "cu",
    name: "Cuba",
    tier: "embargo",
    scope: "national",
    authority: [
      {
        body: "OFAC",
        instrument: "Cuban Assets Control Regulations, 31 CFR Part 515",
        url: "https://www.ecfr.gov/current/title-31/part-515",
      },
      {
        body: "OFAC",
        instrument: "Cuba Sanctions programme page",
        url: `${OFAC_PROGRAMMES}/cuba-sanctions`,
      },
    ],
    reviewed: "2026-08-22",
    status: "settled",
  },
  {
    id: "ir",
    name: "Iran",
    tier: "embargo",
    scope: "national",
    authority: [
      {
        body: "OFAC",
        instrument: "Iranian Transactions and Sanctions Regulations, 31 CFR Part 560",
        url: "https://www.ecfr.gov/current/title-31/part-560",
      },
      {
        body: "OFAC",
        instrument: "Iran Sanctions programme page",
        url: `${OFAC_PROGRAMMES}/iran-sanctions`,
      },
    ],
    reviewed: "2026-08-22",
    status: "settled",
  },
  {
    id: "kp",
    name: "North Korea",
    tier: "embargo",
    scope: "national",
    authority: [
      {
        body: "OFAC",
        instrument: "North Korea Sanctions Regulations, 31 CFR Part 510",
        url: "https://www.ecfr.gov/current/title-31/part-510",
      },
      {
        body: "OFAC",
        instrument: "North Korea Sanctions programme page",
        url: `${OFAC_PROGRAMMES}/north-korea-sanctions`,
      },
    ],
    reviewed: "2026-08-22",
    status: "settled",
  },
  {
    id: "sy",
    name: "Syria",
    tier: "embargo",
    scope: "national",
    authority: [
      {
        body: "OFAC",
        instrument: "Sanctions Programs and Country Information (programme index)",
        url: OFAC_PROGRAMMES,
        note: "Cited in place of a programme page, because there is currently no Syria page to cite.",
      },
    ],
    reviewed: "2026-08-22",
    status: "open-question",
    note:
      "Filed under embargo because the memo files it there, and because the conservative reading " +
      "is the safe one while the question is open — not because the scope was confirmed. Two things " +
      "were observed on 2026-08-22: OFAC's programme index carried no Syria page, and 31 CFR Part 542 " +
      "(the Syrian Sanctions Regulations) was absent from the current CFR. Neither observation is a " +
      "conclusion about what is now permitted.",
    todo:
      "TODO(lawyer): establish what the Syria programme covers after the 2025 changes, and re-tier " +
      "this entry on the answer.",
  },
  {
    id: "by",
    name: "Belarus",
    tier: "category-prohibited",
    scope: "national",
    authority: [
      {
        body: "BIS",
        instrument: "15 CFR 746.8(a)(8) — Sanctions against Russia and Belarus",
        url: "https://www.ecfr.gov/current/title-15/section-746.8",
        note: "Names project management software, and explicitly its updates.",
      },
      {
        body: "OFAC",
        instrument: "Belarus Sanctions programme page",
        url: `${OFAC_PROGRAMMES}/belarus-sanctions`,
      },
    ],
    reviewed: "2026-08-22",
    status: "open-question",
    note:
      "The memo groups Belarus with Russia. Of the instruments cited below, only the BIS section is " +
      "confirmed to reach Belarus by its own title; the OFAC determination and the EU annex cited on " +
      "the Russia entry are Russia instruments and are deliberately not restated here.",
    todo:
      "TODO(lawyer): two questions — whether ADI falls inside the prohibited software category at " +
      "all (the same question as Russia), and which OFAC and EU instruments reach Belarus " +
      "specifically.",
  },
  {
    id: "ru",
    name: "Russia",
    tier: "category-prohibited",
    scope: "national",
    authority: [
      {
        body: "OFAC",
        instrument: "Determination under E.O. 14071 §1(a)(ii)",
        url: `${OFAC_PROGRAMMES}/russian-harmful-foreign-activities-sanctions`,
        effective: "2024-09-12",
        note: "FAQ 1187 names project management.",
      },
      {
        body: "BIS",
        instrument: "15 CFR 746.8(a)(8) — Sanctions against Russia and Belarus",
        url: "https://www.ecfr.gov/current/title-15/section-746.8",
        note: "Names project management software, and explicitly its updates.",
      },
      {
        body: "EU",
        instrument: "Regulation (EU) 833/2014, Annex XXXIX",
        url: "https://eur-lex.europa.eu/eli/reg/2014/833/oj",
        note: "The same line item as the BIS section.",
      },
    ],
    reviewed: "2026-08-22",
    status: "open-question",
    note:
      "Russia is not comprehensively embargoed. What is prohibited is a category of software, and " +
      "whether ADI — a task tree with roles and orchestration — sits inside that category is the " +
      "open question, not a settled exclusion.",
    todo:
      "TODO(lawyer): classify ADI against the prohibited software category (export classification " +
      "already filed). This question decides this tier and nothing else.",
  },
  {
    id: "ua",
    name: "Ukraine",
    tier: "screen-only",
    scope: "regional",
    authority: [
      {
        body: "OFAC",
        instrument: "Ukraine-/Russia-related Sanctions programme page",
        url: `${OFAC_PROGRAMMES}/ukraine-russia-related-sanctions`,
        note: "The programme the regional determinations below are issued under.",
      },
    ],
    reviewed: "2026-08-22",
    status: "settled",
    note:
      "Ukraine is a partner jurisdiction and carries no national restriction. It is listed here only " +
      "because the occupied territories are carved out of it — excluding the country wholesale would " +
      "be both wrong and insulting, and omitting the carve-out would be a compliance failure.",
    regions: [
      {
        id: "crimea",
        name: "Crimea",
        tier: "embargo",
        authority: [
          {
            body: "OFAC",
            instrument: "E.O. 13685 (Crimea region of Ukraine)",
            url: "https://www.federalregister.gov/documents/2014/12/24/2014-30323/blocking-property-of-certain-persons-and-prohibiting-certain-transactions-with-respect-to-the-crimea",
            effective: "2014-12-19",
          },
        ],
        status: "settled",
      },
      {
        id: "donetsk-luhansk",
        name: "The Donetsk and Luhansk areas",
        tier: "embargo",
        authority: [
          {
            body: "OFAC",
            instrument: "E.O. 14065 (Covered Regions of Ukraine)",
            url: "https://www.federalregister.gov/documents/2022/02/23/2022-04020/blocking-property-of-certain-persons-and-prohibiting-certain-transactions-with-respect-to-continued",
            effective: "2022-02-21",
          },
        ],
        status: "settled",
      },
      {
        id: "further-covered-regions",
        name: "Any further region designated a Covered Region under E.O. 14065",
        tier: "embargo",
        authority: [
          {
            body: "OFAC",
            instrument: "E.O. 14065 (Covered Regions of Ukraine), and determinations under it",
            url: "https://www.federalregister.gov/documents/2022/02/23/2022-04020/blocking-property-of-certain-persons-and-prohibiting-certain-transactions-with-respect-to-continued",
            effective: "2022-02-21",
          },
        ],
        status: "open-question",
        note:
          "The memo records that later determinations cover further regions. Which regions are " +
          "currently designated was not verified for this dataset, so the entry names the mechanism " +
          "rather than guessing at the list.",
        todo:
          "TODO(lawyer): confirm the current list of Covered Regions and name them here instead of " +
          "this catch-all.",
      },
    ],
  },
  {
    id: "rest-of-world",
    name: "Everywhere else",
    tier: "screen-only",
    scope: "national",
    isDefault: true,
    authority: [
      {
        body: "OFAC",
        instrument: "Specially Designated Nationals and Blocked Persons (SDN) List",
        url: "https://ofac.treasury.gov/specially-designated-nationals-and-blocked-persons-list-sdn-human-readable-lists",
      },
    ],
    reviewed: "2026-08-22",
    status: "settled",
    note:
      "The check that still applies here is person-level and country-independent: a designated " +
      "person or entity may not be dealt with even in a jurisdiction where the software is fully " +
      "available.",
  },
];

/**
 * The telemetry position, as facts rather than promises.
 *
 * Kept in the dataset rather than in one document because the privacy policy will need the same
 * sentences. The `verified` date is what makes it safe to generate: re-check it against the tree
 * before regenerating, and never move the date without doing so.
 */
export const TELEMETRY = {
  verified: "2026-08-22",
  method: "reading the source tree of this repository",
  facts: [
    "ADI contains no telemetry and no phone-home of any kind.",
    "If anything of that kind is ever added, its entire payload is documented before it ships.",
    "Anything of that kind has an obvious way to turn it off.",
    "Everything ADI holds lives in `~/.adi/mono`, on the machine you run it on.",
  ],
};

export const DATASET: Dataset = { jurisdictions: JURISDICTIONS, telemetry: TELEMETRY };
