/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";
import { fleetMachines, localApps, lockMachine, unlockMachine } from "./home-apps";

test("local catalog excludes archived and invalid records while keeping live app details", () => {
  const apps = localApps({ dashboards: [
    { id: "editor", name: "Editor", description: "Write together", host: "editor.adi", archived_at: null },
    { id: "archived", host: "archived.adi", archived_at: "2026-11-01T10:00:00Z" },
    { id: "legacy-archived", host: "legacy.adi", archived_at: 0 },
    null, {}, { id: " " },
  ] });
  assert.equal(apps.length, 1);
  assert.equal(apps[0]?.name, "Editor");
  assert.equal(apps[0]?.description, "Write together");
  assert.equal(apps[0]?.href, "http://editor.adi/");
});

test("stopped local apps retain their host link so visiting can wake them", () => {
  const apps = localApps({ dashboards: [
    { id: "sleeping", host: "sleeping.adi", frontend_running: false, backend_running: false },
    { id: "port-only", frontend_port: 4312, frontend_running: false },
    { id: "running-port", frontend_port: 4313, frontend_running: true },
  ] });
  assert.equal(apps[0]?.href, "http://sleeping.adi/");
  assert.equal(apps[1]?.href, null);
  assert.equal(apps[2]?.href, "http://127.0.0.1:4313/");
});

test("the local design-system tile alone enters this showcase's component gallery", () => {
  const apps = localApps({ dashboards: [
    { id: "design-system", host: "design-system.adi", frontend_running: false },
    { id: "other-showcase", host: "design-system.adi" },
    { id: "design-system", host: "design-system.example" },
  ] });
  assert.equal(apps[0]?.href, "/library");
  assert.equal(apps[1]?.href, "http://design-system.adi/");
  assert.equal(apps[2]?.href, "http://design-system.example/");
});

test("remote copies keep their node's URLs, including stopped apps and Design System", () => {
  const machines = fleetMachines({ nodes: [{ node: "studio", dashboards: [
    { id: "design-system", url: "https://design-system.studio.n.adi/", allowed: true, frontend_running: false },
    { id: "editor", url: "http://editor.studio.n.adi/documents?q=latest#draft", allowed: true, frontend_running: false },
  ] }] });
  assert.equal(machines[0]?.apps[0]?.href, "https://design-system.studio.n.adi/");
  assert.equal(machines[0]?.apps[1]?.href, "http://editor.studio.n.adi/documents?q=latest#draft");
  assert.equal(machines[0]?.apps[0]?.source, "studio");
});

test("identical app identifiers on different machines have distinct stable keys", () => {
  const local = localApps({ dashboards: [{ id: "editor", host: "editor.adi" }] });
  const payload = { nodes: ["studio", "server"].map((node) => ({
    node, dashboards: [{ id: "editor", url: `http://editor.${node}.n.adi/`, allowed: true }],
  })) };
  const keys = [...local, ...fleetMachines(payload).flatMap((machine) => machine.apps)].map((app) => app.key);
  assert.equal(new Set(keys).size, 3);
  assert.deepEqual(fleetMachines(payload).flatMap((machine) => machine.apps.map((app) => app.key)), keys.slice(1));
});

test("locked and unreachable machines remain visible without presenting stale app links", () => {
  const machines = fleetMachines({ nodes: [
    { node: "locked", locked: true, dashboards: [{ id: "editor", url: "http://editor.locked.n.adi/", allowed: true }] },
    { node: "offline", error: "Machine is unreachable", dashboards: [{ id: "editor", url: "http://editor.offline.n.adi/", allowed: true }] },
  ] });
  assert.equal(machines.length, 2);
  assert.deepEqual(machines.map((machine) => machine.locked), [true, false]);
  assert.match(machines[0]?.issue ?? "", /Unlock/);
  assert.equal(machines[1]?.issue, "Machine is unreachable");
  assert.deepEqual(machines.map((machine) => machine.apps), [[], []]);
  assert.deepEqual(machines.map((machine) => machine.href), ["http://app.locked.n.adi/", "http://app.offline.n.adi/"]);
});

test("a malformed machine listing stays visible with its identity and recovery link", () => {
  const machines = fleetMachines({ nodes: [
    { node: "missing" },
    { node: "malformed", dashboards: { editor: {} } },
    { node: "healthy", dashboards: [{ id: "editor", url: "http://editor.healthy.n.adi/", allowed: true }] },
  ] });
  assert.deepEqual(machines.map((machine) => machine.id), ["missing", "malformed", "healthy"]);
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
  const known = localApps({ dashboards: [
    { id: "design-system", icon: "constructor" },
    { id: "__proto__", icon: "blocks" },
  ] });
  assert.notEqual(known[0]?.icon, null);
  assert.equal(known[0]?.icon, known[1]?.icon);
});

test("apps without remote access open their machine panel instead of the app", () => {
  const machine = fleetMachines({ nodes: [{ node: "studio", dashboards: [
    { id: "restricted", url: "https://restricted.studio.n.adi/private", allowed: false },
    { id: "unspecified", url: "http://unspecified.studio.n.adi/" },
  ] }] })[0];
  assert.ok(machine);
  for (const app of machine.apps) {
    assert.equal(app.href, "http://app.studio.n.adi/");
    assert.match(app.notice ?? "", /Access required/);
  }
});

test("missing, malformed, cross-machine and unsafe addresses are never actionable app links", () => {
  const invalid = [undefined, "", "not a URL", "/relative", "javascript:alert(1)", "ftp://files.studio.n.adi/",
    "https://editor.other.n.adi/", "https://editor.studio.n.adi.attacker.example/", "https://user:password@editor.studio.n.adi/"];
  const machine = fleetMachines({ nodes: [{ node: "studio", dashboards: invalid.map((url, index) => ({
    id: `invalid-${index}`, url, allowed: true,
  })) }] })[0];
  assert.equal(machine?.apps.length, invalid.length);
  for (const app of machine?.apps ?? []) {
    assert.equal(app.href, null);
    assert.ok(app.notice);
  }
  for (const app of localApps({ dashboards: [
    { id: "missing" }, { id: "scheme-host", host: "https://editor.adi" }, { id: "path-host", host: "editor.adi/path" },
    { id: "bad-port", frontend_running: true, frontend_port: 70000 },
  ] })) {
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
  const machines = fleetMachines({ nodes: [
    { node: "studio", locked: true, error: "studio refused the password", dashboards: [] },
    { node: "server", locked: false, error: "server did not answer in time", dashboards: [] },
  ] });
  assert.equal(machines[0]?.locked, true);
  assert.equal(machines[0]?.issue, "studio refused the password");
  assert.equal(machines[1]?.locked, false);
  assert.equal(machines[1]?.issue, "server did not answer in time");
});

test("unlock posts credentials with an exact password and returns the refreshed machine apps", async () => {
  const originalFetch = globalThis.fetch;
  const calls: { input: RequestInfo | URL; init?: RequestInit }[] = [];
  const listing = { nodes: [{ node: "studio", locked: false, dashboards: [
    { id: "editor", name: "Editor", url: "http://editor.studio.n.adi/", allowed: true },
  ] }] };
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ input, init });
    return Response.json(listing);
  }) as typeof fetch;
  try {
    const signal = new AbortController().signal;
    for (const [username, expectedUsername] of [["  u-workstation  ", "u-workstation"], [" \t ", "adi"]]) {
      const machines = await unlockMachine("studio", username, "  password with spaces \t", signal);
      const call = calls.at(-1);
      assert.equal(call?.input, "/api/app/fleet/dashboards/unlock");
      assert.equal(call?.init?.method, "POST");
      assert.deepEqual(call?.init?.headers, { "Content-Type": "application/json" });
      assert.equal(call?.init?.signal, signal);
      assert.deepEqual(JSON.parse(String(call?.init?.body)), {
        node: "studio", username: expectedUsername, password: "  password with spaces \t",
      });
      assert.deepEqual(machines, fleetMachines(listing));
      assert.equal(machines[0]?.locked, false);
      assert.equal(machines[0]?.apps[0]?.href, "http://editor.studio.n.adi/");
    }
    assert.equal(calls.length, 2);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("lock forgets only the named machine and parses the refreshed locked listing", async () => {
  const originalFetch = globalThis.fetch;
  const signal = new AbortController().signal;
  let calls = 0;
  const listing = { nodes: [
    { node: "studio", locked: true, dashboards: [] },
    { node: "server", locked: false, dashboards: [{ id: "editor", url: "http://editor.server.n.adi/", allowed: true }] },
  ] };
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    calls += 1;
    assert.equal(input, "/api/app/fleet/dashboards/forget");
    assert.equal(init?.method, "POST");
    assert.deepEqual(init?.headers, { "Content-Type": "application/json" });
    assert.equal(init?.signal, signal);
    assert.deepEqual(JSON.parse(String(init?.body)), { petname: "studio" });
    return Response.json(listing);
  }) as typeof fetch;
  try {
    const machines = await lockMachine("studio", signal);
    assert.equal(calls, 1);
    assert.deepEqual(machines, fleetMachines(listing));
    assert.deepEqual(machines.map((machine) => machine.locked), [true, false]);
    assert.equal(machines[1]?.apps[0]?.href, "http://editor.server.n.adi/");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("lock and unlock surface rejected credentials and successful HTTP responses containing API errors", async () => {
  const originalFetch = globalThis.fetch;
  try {
    for (const status of [401, 200]) {
      globalThis.fetch = (async () => Response.json({ ok: false, error: "studio refused the password" }, { status })) as typeof fetch;
      await assert.rejects(unlockMachine("studio", "adi", "wrong password"), /studio refused the password/);
      await assert.rejects(lockMachine("studio"), /studio refused the password/);
    }
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("invalid machine names and an empty password fail before making any network request", async () => {
  const originalFetch = globalThis.fetch;
  let calls = 0;
  globalThis.fetch = (async () => {
    calls += 1;
    throw new Error("Unexpected network request");
  }) as typeof fetch;
  try {
    for (const node of ["", " studio ", "Studio", "studio.other", "../studio", "-studio", "studio-", "a".repeat(64)]) {
      await assert.rejects(unlockMachine(node, "adi", "password"), /valid paired machine/);
      await assert.rejects(lockMachine(node), /valid paired machine/);
    }
    await assert.rejects(unlockMachine("studio", "adi", ""), /Enter the machine password/);
    assert.equal(calls, 0);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
