/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  fleetMachines,
  homeCatalogKeys,
  localApps,
  saveFleetMachines,
  saveLocalApps,
  savedFleetMachines,
  savedLocalApps,
} from "./home-apps";

test("local catalog excludes archived and invalid records while keeping live app details", () => {
  const apps = localApps({
    dashboards: [
      {
        id: "editor",
        name: "Editor",
        description: "Write together",
        host: "editor.adi",
        archived_at: null,
      },
      { id: "archived", host: "archived.adi", archived_at: "2026-11-01T10:00:00Z" },
      { id: "legacy-archived", host: "legacy.adi", archived_at: 0 },
      null,
      {},
      { id: " " },
    ],
  });
  assert.equal(apps.length, 1);
  assert.equal(apps[0]?.name, "Editor");
  assert.equal(apps[0]?.description, "Write together");
  assert.equal(apps[0]?.href, "http://editor.adi/");
});

test("stopped local apps retain their host link so visiting can wake them", () => {
  const apps = localApps({
    dashboards: [
      { id: "sleeping", host: "sleeping.adi", frontend_running: false, backend_running: false },
      { id: "port-only", frontend_port: 4312, frontend_running: false },
      { id: "running-port", frontend_port: 4313, frontend_running: true },
    ],
  });
  assert.equal(apps[0]?.href, "http://sleeping.adi/");
  assert.equal(apps[1]?.href, null);
  assert.equal(apps[2]?.href, "http://127.0.0.1:4313/");
});

test("the local design-system tile alone enters this showcase's component gallery", () => {
  const apps = localApps({
    dashboards: [
      { id: "design-system", host: "design-system.adi", frontend_running: false },
      { id: "other-showcase", host: "design-system.adi" },
      { id: "design-system", host: "design-system.example" },
    ],
  });
  assert.equal(apps[0]?.href, "/library");
  assert.equal(apps[1]?.href, "http://design-system.adi/");
  assert.equal(apps[2]?.href, "http://design-system.example/");
});

test("remote copies keep their node's URLs, including stopped apps and Design System", () => {
  const machines = fleetMachines({
    nodes: [
      {
        node: "studio",
        dashboards: [
          {
            id: "design-system",
            url: "https://design-system.studio.n.adi/",
            allowed: true,
            frontend_running: false,
          },
          {
            id: "editor",
            url: "http://editor.studio.n.adi/documents?q=latest#draft",
            allowed: true,
            frontend_running: false,
          },
        ],
      },
    ],
  });
  assert.equal(machines[0]?.apps[0]?.href, "https://design-system.studio.n.adi/");
  assert.equal(machines[0]?.apps[1]?.href, "http://editor.studio.n.adi/documents?q=latest#draft");
  assert.equal(machines[0]?.apps[0]?.source, "studio");
});

test("identical app identifiers on different machines have distinct stable keys", () => {
  const local = localApps({ dashboards: [{ id: "editor", host: "editor.adi" }] });
  const payload = {
    nodes: ["studio", "server"].map((node) => ({
      node,
      dashboards: [{ id: "editor", url: `http://editor.${node}.n.adi/`, allowed: true }],
    })),
  };
  const keys = [...local, ...fleetMachines(payload).flatMap((machine) => machine.apps)].map(
    (app) => app.key,
  );
  assert.equal(new Set(keys).size, 3);
  assert.deepEqual(
    fleetMachines(payload).flatMap((machine) => machine.apps.map((app) => app.key)),
    keys.slice(1),
  );
});

test("locked and unreachable machines remain visible without presenting stale app links", () => {
  const machines = fleetMachines({
    nodes: [
      {
        node: "locked",
        locked: true,
        dashboards: [{ id: "editor", url: "http://editor.locked.n.adi/", allowed: true }],
      },
      {
        node: "offline",
        error: "Machine is unreachable",
        dashboards: [{ id: "editor", url: "http://editor.offline.n.adi/", allowed: true }],
      },
    ],
  });
  assert.equal(machines.length, 2);
  assert.deepEqual(
    machines.map((machine) => machine.locked),
    [true, false],
  );
  assert.match(machines[0]?.issue ?? "", /Unlock/);
  assert.equal(machines[1]?.issue, "Machine is unreachable");
  assert.deepEqual(
    machines.map((machine) => machine.apps),
    [[], []],
  );
  assert.deepEqual(
    machines.map((machine) => machine.href),
    ["http://app.locked.n.adi/", "http://app.offline.n.adi/"],
  );
});

test("a malformed machine listing stays visible with its identity and recovery link", () => {
  const machines = fleetMachines({
    nodes: [
      { node: "missing" },
      { node: "malformed", dashboards: { editor: {} } },
      {
        node: "healthy",
        dashboards: [{ id: "editor", url: "http://editor.healthy.n.adi/", allowed: true }],
      },
    ],
  });
  assert.deepEqual(
    machines.map((machine) => machine.id),
    ["missing", "malformed", "healthy"],
  );
  for (const machine of machines.slice(0, 2)) {
    assert.equal(machine.label, machine.id);
    assert.match(machine.issue ?? "", /app catalog could not be read/);
    assert.deepEqual(machine.apps, []);
    assert.equal(machine.href, `http://app.${machine.id}.n.adi/`);
  }
  assert.equal(machines[2]?.issue, undefined);
  assert.equal(machines[2]?.apps[0]?.source, "healthy");
  assert.equal(machines[2]?.apps[0]?.href, "http://editor.healthy.n.adi/");
});

test("unrecognized icon and app names cannot resolve inherited object properties", () => {
  const rows = [
    { id: "constructor", icon: "constructor" },
    { id: "__proto__", icon: "__proto__" },
    { id: "unknown-icon", icon: "unlisted-icon" },
    { id: "missing-icon" },
  ];
  const local = localApps({ dashboards: rows });
  const remote = fleetMachines({ nodes: [{ node: "studio", dashboards: rows }] })[0]?.apps;
  assert.equal(local.length, rows.length);
  assert.equal(remote?.length, rows.length);
  for (const app of [...local, ...(remote ?? [])]) assert.equal(app.icon, null);
  const known = localApps({
    dashboards: [
      { id: "design-system", icon: "constructor" },
      { id: "__proto__", icon: "blocks" },
    ],
  });
  assert.notEqual(known[0]?.icon, null);
  assert.equal(known[0]?.icon, known[1]?.icon);
});

test("apps without remote access open their machine panel instead of the app", () => {
  const machine = fleetMachines({
    nodes: [
      {
        node: "studio",
        dashboards: [
          { id: "restricted", url: "https://restricted.studio.n.adi/private", allowed: false },
          { id: "unspecified", url: "http://unspecified.studio.n.adi/" },
        ],
      },
    ],
  })[0];
  assert.ok(machine);
  for (const app of machine.apps) {
    assert.equal(app.href, "http://app.studio.n.adi/");
    assert.match(app.notice ?? "", /Access required/);
  }
});

test("missing, malformed, cross-machine and unsafe addresses are never actionable app links", () => {
  const invalid = [
    undefined,
    "",
    "not a URL",
    "/relative",
    "javascript:alert(1)",
    "ftp://files.studio.n.adi/",
    "https://editor.other.n.adi/",
    "https://editor.studio.n.adi.attacker.example/",
    "https://user:password@editor.studio.n.adi/",
  ];
  const machine = fleetMachines({
    nodes: [
      {
        node: "studio",
        dashboards: invalid.map((url, index) => ({
          id: `invalid-${index}`,
          url,
          allowed: true,
        })),
      },
    ],
  })[0];
  assert.equal(machine?.apps.length, invalid.length);
  for (const app of machine?.apps ?? []) {
    assert.equal(app.href, null);
    assert.ok(app.notice);
  }
  for (const app of localApps({
    dashboards: [
      { id: "missing" },
      { id: "scheme-host", host: "https://editor.adi" },
      { id: "path-host", host: "editor.adi/path" },
      { id: "bad-port", frontend_running: true, frontend_port: 70000 },
    ],
  })) {
    assert.equal(app.href, null);
    assert.ok(app.notice);
  }
});

test("malformed catalog envelopes fail explicitly instead of looking like empty machines", () => {
  for (const payload of [null, [], "catalog", {}, { dashboards: null }, { dashboards: {} }]) {
    assert.throws(() => localApps(payload), /app catalog could not be read/);
  }
  for (const payload of [null, [], "catalog", {}, { nodes: null }, { nodes: {} }]) {
    assert.throws(() => fleetMachines(payload), /app catalog could not be read/);
  }
  assert.deepEqual(localApps({ dashboards: [] }), []);
  assert.deepEqual(fleetMachines({ nodes: [] }), []);
});

test("machines with rejected stored credentials stay locked and retain the server's error", () => {
  const machines = fleetMachines({
    nodes: [
      { node: "studio", locked: true, error: "studio refused the password", dashboards: [] },
      { node: "server", locked: false, error: "server did not answer in time", dashboards: [] },
    ],
  });
  assert.equal(machines[0]?.locked, true);
  assert.equal(machines[0]?.issue, "studio refused the password");
  assert.equal(machines[1]?.locked, false);
  assert.equal(machines[1]?.issue, "server did not answer in time");
});

function memoryStorage(): Storage {
  const items = new Map<string, string>();
  return {
    get length() {
      return items.size;
    },
    clear: () => items.clear(),
    getItem: (key) => items.get(key) ?? null,
    key: (index) => [...items.keys()][index] ?? null,
    removeItem: (key) => {
      items.delete(key);
    },
    setItem: (key, value) => {
      items.set(key, String(value));
    },
  };
}

test("a saved catalog restores the apps a live read listed, with the time it was read", () => {
  const storage = memoryStorage();
  const local = {
    dashboards: [
      {
        id: "design-system",
        name: "Design System",
        description: "Shared components",
        host: "design-system.adi",
        icon: "blocks",
        archived_at: null,
      },
      { id: "port-only", frontend_port: 4313, frontend_running: true },
      { id: "archived", host: "archived.adi", archived_at: 1000 },
      { id: "no-address" },
    ],
  };
  const fleet = {
    nodes: [
      {
        node: "studio",
        locked: false,
        error: null,
        dashboards: [
          {
            id: "crm",
            name: "CRM",
            description: "Accounts",
            url: "http://crm.studio.n.adi/",
            allowed: true,
            icon: "wallet",
          },
          { id: "restricted", url: "http://restricted.studio.n.adi/", allowed: false },
        ],
      },
      { node: "empty", dashboards: [] },
    ],
  };
  assert.equal(savedLocalApps(storage), undefined);
  assert.equal(savedFleetMachines(storage), undefined);
  const apps = saveLocalApps(local, storage, 1000);
  const machines = saveFleetMachines(fleet, storage, 2000);
  assert.deepEqual(savedLocalApps(storage), { data: apps, savedAt: 1000 });
  assert.deepEqual(savedFleetMachines(storage), {
    data: machines.map((machine) => ({ ...machine, saved: true })),
    savedAt: 2000,
  });
  assert.deepEqual(
    apps.map((app) => app.href),
    ["/library", "http://127.0.0.1:4313/", null],
  );
  assert.notEqual(savedLocalApps(storage)?.data[0]?.icon, null);
  assert.equal(
    machines.some((machine) => machine.saved),
    false,
    "a live read is never marked as saved",
  );
});

test("a saved catalog keeps only the fields Home reads", () => {
  const storage = memoryStorage();
  saveLocalApps(
    {
      dashboards: [
        {
          id: "editor",
          host: "editor.adi",
          dir: "/Users/someone/editor",
          project: "private",
          routes: ["/x"],
          backend_port: 4000,
        },
      ],
      token: "secret",
    },
    storage,
  );
  saveFleetMachines(
    {
      nodes: [
        {
          node: "studio",
          password: "secret",
          dashboards: [
            {
              id: "crm",
              url: "http://crm.studio.n.adi/",
              allowed: true,
              service: "crm",
              dir: "/srv/crm",
            },
          ],
        },
      ],
    },
    storage,
  );
  const stored = Object.values(homeCatalogKeys)
    .map((key) => storage.getItem(key) ?? "")
    .join("\n");
  for (const kept of ["editor.adi", "http://crm.studio.n.adi/"]) assert.ok(stored.includes(kept));
  for (const dropped of [
    "/Users/someone",
    "private",
    "routes",
    "backend_port",
    "secret",
    "/srv/crm",
    "service",
  ])
    assert.ok(!stored.includes(dropped), `${dropped} is not stored`);
});

test("a machine that was locked or failing when it was read is restored without apps", () => {
  const storage = memoryStorage();
  const row = { id: "editor", url: "http://editor.studio.n.adi/", allowed: true };
  saveFleetMachines(
    {
      nodes: [
        { node: "locked", locked: true, dashboards: [row] },
        { node: "offline", error: "Machine is unreachable", dashboards: [row] },
        { node: "malformed", dashboards: { editor: row } },
        null,
      ],
    },
    storage,
  );
  assert.ok(!(storage.getItem(homeCatalogKeys.fleet) ?? "").includes("editor"));
  const machines = savedFleetMachines(storage)?.data ?? [];
  assert.deepEqual(
    machines.map((machine) => [machine.id, machine.locked, machine.apps.length, machine.saved]),
    [
      ["locked", true, 0, true],
      ["offline", false, 0, true],
      ["malformed", false, 0, true],
    ],
  );
  assert.match(machines[0]?.issue ?? "", /Unlock/);
  assert.equal(machines[1]?.issue, "Machine is unreachable");
  assert.match(machines[2]?.issue ?? "", /app catalog could not be read/);
});

test("a failed read saves nothing and leaves the last good catalog in place", () => {
  const storage = memoryStorage();
  saveLocalApps({ dashboards: [{ id: "editor", host: "editor.adi" }] }, storage, 1000);
  assert.throws(
    () => saveLocalApps({ error: "unavailable" }, storage, 2000),
    /app catalog could not be read/,
  );
  assert.throws(
    () => saveFleetMachines({ error: "unavailable" }, storage, 2000),
    /app catalog could not be read/,
  );
  assert.equal(savedLocalApps(storage)?.savedAt, 1000);
  assert.equal(savedFleetMachines(storage), undefined);
});

test("stored addresses are checked again, so an edited store cannot add a link the server would not send", () => {
  const storage = memoryStorage();
  storage.setItem(
    homeCatalogKeys.local,
    JSON.stringify({
      savedAt: 1000,
      payload: {
        dashboards: [
          { id: "bad", host: "javascript:alert(1)" },
          { id: "path", host: "editor.adi/path" },
        ],
      },
    }),
  );
  storage.setItem(
    homeCatalogKeys.fleet,
    JSON.stringify({
      savedAt: 1000,
      payload: {
        nodes: [
          {
            node: "studio",
            dashboards: [
              { id: "script", url: "javascript:alert(1)", allowed: true },
              { id: "elsewhere", url: "https://editor.attacker.example/", allowed: true },
            ],
          },
        ],
      },
    }),
  );
  for (const app of [
    ...(savedLocalApps(storage)?.data ?? []),
    ...(savedFleetMachines(storage)?.data[0]?.apps ?? []),
  ])
    assert.equal(app.href, null);
  assert.equal(savedLocalApps(storage)?.data.length, 2);
  assert.equal(savedFleetMachines(storage)?.data[0]?.apps.length, 2);
});

test("an unreadable, outdated or unavailable store means an empty start, not an error", () => {
  const storage = memoryStorage();
  for (const value of [
    "",
    "{",
    "null",
    "[]",
    "{}",
    JSON.stringify({ savedAt: "yesterday", payload: { dashboards: [] } }),
    JSON.stringify({ savedAt: 1000 }),
    JSON.stringify({ savedAt: 1000, payload: { dashboards: {} } }),
  ]) {
    storage.setItem(homeCatalogKeys.local, value);
    storage.setItem(homeCatalogKeys.fleet, value);
    assert.equal(savedLocalApps(storage), undefined, value);
    assert.equal(savedFleetMachines(storage), undefined, value);
  }
  const refusing = {
    ...memoryStorage(),
    getItem: () => {
      throw new Error("denied");
    },
    setItem: () => {
      throw new Error("quota");
    },
  } as Storage;
  assert.equal(savedLocalApps(refusing), undefined);
  assert.deepEqual(saveLocalApps({ dashboards: [] }, refusing), []);
  assert.deepEqual(saveLocalApps({ dashboards: [] }, undefined), []);
  assert.equal(savedFleetMachines(undefined), undefined);
});
