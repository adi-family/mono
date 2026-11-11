/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";
import { installHref, marketplaceKey, marketplaceProjectsKey } from "./marketplace";
import { normalizeMarketplaceSource, readMarketplaceSource } from "./marketplace-source";

test("marketplace reads are cached per machine", () => {
  assert.deepEqual(marketplaceKey(), ["marketplace-state", ""]);
  assert.deepEqual(marketplaceKey("studio"), ["marketplace-state", "studio"]);
  assert.notDeepEqual(marketplaceProjectsKey("studio"), marketplaceProjectsKey());
});

test("a started copy opens on its own machine, through the mesh for a paired one", () => {
  assert.equal(installHref("pulse.adi"), "http://pulse.adi");
  assert.equal(installHref("127.0.0.1:4173"), "http://127.0.0.1:4173");
  assert.equal(installHref("pulse.adi", "studio"), "http://pulse.studio.n.adi/");
  assert.equal(installHref("app.nosh1.adi", "hetzner-nosh"), "http://app.nosh1.hetzner-nosh.n.adi/");
  // Only an `.adi` name has a mesh address on another machine.
  assert.equal(installHref("127.0.0.1:4173", "studio"), null);
  assert.equal(installHref(".adi", "studio"), null);
  assert.equal(installHref("", "studio"), null);
  assert.equal(installHref(null), null);
});

test("the remembered marketplace machine is a paired petname, else this machine", () => {
  assert.equal(normalizeMarketplaceSource("studio"), "studio");
  assert.equal(normalizeMarketplaceSource(""), undefined);
  for (const invalid of [null, 7, "../x", "Studio", "a".repeat(64), { node: "studio" }]) assert.equal(normalizeMarketplaceSource(invalid), undefined);
  // Outside a browser there is nothing remembered.
  assert.equal(readMarketplaceSource(), undefined);
});
