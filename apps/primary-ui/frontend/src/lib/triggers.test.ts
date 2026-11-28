/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  addEventPattern,
  applyPreset,
  codeHint,
  customRelayUrl,
  draftOf,
  draftProblem,
  emptyDraft,
  enabledBody,
  filterTriggers,
  formatAge,
  formatUptime,
  isCustomRelay,
  parseEventPatterns,
  relayChoice,
  relayKeyKey,
  relayPublicUrl,
  resolveRelayUrl,
  saveBody,
  settingFields,
  settingsProjectsKey,
  triggerHref,
  triggerLogKey,
  triggerStatus,
  triggersKey,
  webhookPath,
  type Trigger,
  type TriggerPreset,
} from "./triggers";
import { readSettingsSource } from "./settings-source";

const base: Trigger = { name: "deploy-hook", kind: "webhook", created_at: 1, updated_at: 2 };
const telegram: TriggerPreset = {
  id: "telegram-notify",
  label: "Telegram notification",
  description: "Send the payload to a chat",
  kind: "webhook",
  runtime: "sh",
  code: "curl …",
  fields: [
    {
      key: "token_env",
      label: "Token variable",
      hint: "the secret's name",
      default: "TELEGRAM_TOKEN",
    },
    { key: "chat_id", label: "Chat" },
  ],
};
const onEvent: TriggerPreset = {
  id: "event",
  label: "On a platform event",
  kind: "event",
  runtime: "ts",
  code: "console.log()",
  events: ["adi.tasks.*"],
};

test("settings reads are cached per machine, and the machine choice defaults to this one", () => {
  assert.deepEqual(triggersKey(), ["settings-triggers", ""]);
  assert.deepEqual(triggersKey("studio"), ["settings-triggers", "studio"]);
  assert.notDeepEqual(settingsProjectsKey("studio"), settingsProjectsKey());
  assert.deepEqual(triggerLogKey("studio", "a"), ["settings-trigger-log", "studio", "a"]);
  assert.equal(readSettingsSource(), undefined);
});

test("a trigger's views keep its name in the query, so any valid name survives a reload", () => {
  assert.equal(triggerHref(), "/settings/triggers");
  assert.equal(triggerHref("deploy-hook"), "/settings/triggers?trigger=deploy-hook");
  assert.equal(triggerHref("a.b_c", "log"), "/settings/triggers?trigger=a.b_c&view=log");
  assert.equal(
    new URLSearchParams(triggerHref("тест & co").split("?")[1]).get("trigger"),
    "тест & co",
  );
  assert.equal(webhookPath("deploy hook"), "/api/hooks/deploy%20hook");
});

test("status reports a supervised process, an armed endpoint, a stopped process or a disabled trigger", () => {
  assert.deepEqual(
    triggerStatus({
      ...base,
      kind: "background",
      enabled: true,
      running: true,
      pid: 42,
      uptime_secs: 97498,
      restarts: 0,
    }),
    { tone: "running", label: "Running", detail: "pid 42 · up 1d 3h" },
  );
  assert.equal(
    triggerStatus({
      ...base,
      kind: "background",
      enabled: true,
      running: true,
      pid: 42,
      uptime_secs: 30,
      restarts: 3,
    }).detail,
    "pid 42 · up 30s · 3 restarts",
  );
  assert.equal(
    triggerStatus({ ...base, kind: "background", enabled: true, running: true, restarts: 1 })
      .detail,
    "1 restart",
  );
  assert.deepEqual(triggerStatus({ ...base, enabled: true }), {
    tone: "listening",
    label: "Listening",
  });
  assert.deepEqual(triggerStatus({ ...base, kind: "event", enabled: true }), {
    tone: "listening",
    label: "Listening",
  });
  assert.deepEqual(triggerStatus({ ...base, kind: "background", enabled: true }), {
    tone: "stopped",
    label: "Stopped",
    detail: "check the log",
  });
  assert.deepEqual(triggerStatus({ ...base, kind: "background", enabled: false }), {
    tone: "disabled",
    label: "Disabled",
  });
  // A disabled trigger fired by hand is running until it exits.
  assert.equal(triggerStatus({ ...base, enabled: false, running: true }).tone, "running");
});

test("durations and ages use the one unit that matters", () => {
  assert.deepEqual([0, 59, 60, 3599, 3600, 5400, 86400, 90000].map(formatUptime), [
    "0s",
    "59s",
    "1m",
    "59m",
    "1h",
    "1h 30m",
    "1d",
    "1d 1h",
  ]);
  const now = 1_800_000_000_000;
  const ago = (seconds: number) => formatAge(now / 1000 - seconds, now);
  assert.deepEqual([0, 44, 60, 3599, 3600, 86399, 86400, 29 * 86400].map(ago), [
    "just now",
    "just now",
    "1m ago",
    "60m ago",
    "1h ago",
    "24h ago",
    "1d ago",
    "29d ago",
  ]);
  assert.match(ago(400 * 86400), /\d{4}/);
});

test("the list search matches every word against name, description, project, kind and events", () => {
  const list: Trigger[] = [
    { ...base, description: "Ship on push" },
    { ...base, name: "adin-events", kind: "event", project: "adin", events: ["adi.**"] },
    {
      ...base,
      name: "watch",
      kind: "background",
      description: "Poll mainnet",
      project: "bugbounty",
    },
  ];
  assert.deepEqual(
    filterTriggers(list, "  ").map((trigger) => trigger.name),
    ["deploy-hook", "adin-events", "watch"],
  );
  assert.deepEqual(
    filterTriggers(list, "ADIN").map((trigger) => trigger.name),
    ["adin-events"],
  );
  assert.deepEqual(
    filterTriggers(list, "background bugbounty").map((trigger) => trigger.name),
    ["watch"],
  );
  assert.deepEqual(
    filterTriggers(list, "adi.**").map((trigger) => trigger.name),
    ["adin-events"],
  );
  assert.deepEqual(filterTriggers(list, "push mainnet"), []);
});

test("event patterns are one per line or comma; adding one never duplicates it", () => {
  assert.deepEqual(parseEventPatterns(" adi.tasks.* ,adi.agents.**\n\n adi.channels.message "), [
    "adi.tasks.*",
    "adi.agents.**",
    "adi.channels.message",
  ]);
  assert.equal(
    addEventPattern("adi.tasks.*, adi.agents.**", "adi.tasks.created"),
    "adi.tasks.*\nadi.agents.**\nadi.tasks.created",
  );
  assert.equal(addEventPattern("adi.tasks.created\n", "adi.tasks.created"), "adi.tasks.created");
  assert.equal(addEventPattern("", "adi.tasks.created"), "adi.tasks.created");
});

test("a preset fills kind, runtime, code, events and setting defaults, and leaves what is the user's", () => {
  const filled = applyPreset({ ...emptyDraft, name: "notify", project: "ops" }, telegram);
  assert.deepEqual(
    {
      name: filled.name,
      project: filled.project,
      kind: filled.kind,
      runtime: filled.runtime,
      code: filled.code,
      preset: filled.preset,
      description: filled.description,
      extra: filled.extra,
    },
    {
      name: "notify",
      project: "ops",
      kind: "webhook",
      runtime: "sh",
      code: "curl …",
      preset: "telegram-notify",
      description: "Send the payload to a chat",
      extra: { token_env: "TELEGRAM_TOKEN" },
    },
  );
  const kept = applyPreset(
    { ...filled, description: "Mine", extra: { token_env: "MY_TOKEN" }, events: "x" },
    onEvent,
  );
  assert.equal(kept.description, "Mine");
  assert.equal(kept.extra.token_env, "MY_TOKEN");
  assert.equal(kept.events, "adi.tasks.*");
  assert.equal(kept.runtime, "ts");
});

test("the settings offered are a webhook's secret, the preset's fields, then anything already set", () => {
  const fields = settingFields(
    { kind: "webhook", preset: "telegram-notify", extra: { interval_secs: "60", chat_id: "1" } },
    [telegram],
  );
  assert.deepEqual(
    fields.map((field) => field.key),
    ["secret", "token_env", "chat_id", "interval_secs"],
  );
  assert.equal(fields[0].secret, true);
  assert.equal(fields[3].custom, true);
  assert.match(fields[3].hint, /\$ADI_INTERVAL_SECS/);
  assert.deepEqual(settingFields({ kind: "background", preset: null, extra: {} }, [telegram]), []);
  // A preset the machine no longer publishes still shows the values the trigger carries.
  assert.deepEqual(
    settingFields({ kind: "background", preset: "gone", extra: { url: "x" } }, [telegram]).map(
      (field) => field.key,
    ),
    ["url"],
  );
});

test("a save submits only offered, non-empty settings and the fields its kind uses", () => {
  const draft = {
    ...applyPreset({ ...emptyDraft, name: " notify ", description: " Ping " }, telegram),
    triggerOn: ["ops"],
    events: "adi.tasks.*",
  };
  draft.extra = { token_env: " TELEGRAM_TOKEN ", chat_id: "  ", secret: "s3cret" };
  assert.deepEqual(saveBody(draft, [telegram]), {
    name: "notify",
    kind: "webhook",
    runtime: "sh",
    code: "curl …",
    preset: "telegram-notify",
    description: "Ping",
    enabled: true,
    project: null,
    extra: { token_env: "TELEGRAM_TOKEN", secret: "s3cret" },
    events: [],
    trigger_on: ["ops"],
  });
  // Moving to another kind drops the project allowlist; a value already typed is never dropped
  // silently, so the secret stays on as a plain setting until it is cleared.
  const background = saveBody({ ...draft, kind: "background", project: "ops" }, [telegram]);
  assert.deepEqual(
    [background.extra, background.trigger_on, background.project],
    [{ token_env: "TELEGRAM_TOKEN", secret: "s3cret" }, [], "ops"],
  );
  // Switching presets leaves the previous preset's untouched defaults behind only if they were set.
  assert.deepEqual(
    saveBody({ ...draft, preset: null, extra: { chat_id: " " } }, [telegram]).extra,
    {},
  );
  assert.deepEqual(saveBody({ ...draft, kind: "event" }, [telegram]).events, ["adi.tasks.*"]);
});

test("an existing trigger round-trips through the form unchanged", () => {
  const stored: Trigger = {
    ...base,
    kind: "background",
    runtime: "",
    code: "while true; do sleep 1; done",
    preset: null,
    description: "Loop",
    enabled: false,
    project: "ops",
    extra: { interval_secs: "21600", enabled: "true" },
    events: [],
    trigger_on: [],
  };
  const body = saveBody(draftOf(stored), []);
  assert.deepEqual(body, {
    name: "deploy-hook",
    kind: "background",
    runtime: "sh",
    code: "while true; do sleep 1; done",
    preset: null,
    description: "Loop",
    enabled: false,
    project: "ops",
    extra: { interval_secs: "21600", enabled: "true" },
    events: [],
    trigger_on: [],
  });
  assert.equal(JSON.stringify(saveBody(draftOf(stored), [])), JSON.stringify(body));
});

test("the enabled switch re-saves the whole definition with only the flag changed", () => {
  const stored: Trigger = {
    ...base,
    kind: "event",
    runtime: "ts",
    code: "x",
    preset: "event",
    description: "d",
    enabled: true,
    project: "adin",
    extra: { a: "1" },
    events: ["adi.**"],
    trigger_on: ["adin"],
  };
  assert.deepEqual(enabledBody(stored, false), {
    name: "deploy-hook",
    kind: "event",
    runtime: "ts",
    code: "x",
    preset: "event",
    description: "d",
    enabled: false,
    project: "adin",
    extra: { a: "1" },
    events: ["adi.**"],
    trigger_on: ["adin"],
  });
  assert.deepEqual(enabledBody(base, true), {
    name: "deploy-hook",
    kind: "webhook",
    runtime: "",
    code: "",
    preset: null,
    description: "",
    enabled: true,
    project: null,
    extra: {},
    events: [],
    trigger_on: [],
  });
});

test("a draft is checked for the problems that would fail or mislead", () => {
  assert.equal(draftProblem(emptyDraft), "A trigger name is required.");
  for (const name of ["a/b", "with space", "..", ".", "a\\b", "q?x"])
    assert.match(
      draftProblem({ ...emptyDraft, name, kind: "webhook" }) ?? "",
      /letters, digits/,
      name,
    );
  for (const name of ["deploy-hook", "hook_2", "a.b", "тест"])
    assert.equal(draftProblem({ ...emptyDraft, name, kind: "webhook" }), null, name);
  assert.equal(draftProblem({ ...emptyDraft, name: "x" }), "Pick how this trigger launches.");
  assert.equal(
    draftProblem({ ...emptyDraft, name: "x", kind: "event", events: " \n" }),
    "An event trigger needs at least one event pattern.",
  );
  assert.equal(
    draftProblem({ ...emptyDraft, name: "x", kind: "event", events: "adi.tasks.*" }),
    null,
  );
});

test("the code hint says how the block is launched and where its input arrives", () => {
  assert.equal(
    codeHint("webhook", "sh"),
    "Run as sh -c; the request body lands in $ADI_PAYLOAD_FILE, its headers in $ADI_HEADERS_FILE; settings arrive as $ADI_<KEY>.",
  );
  assert.equal(
    codeHint("event", "ts"),
    "Run with bun; the event lands in $ADI_PAYLOAD, its name in $ADI_EVENT; settings arrive as $ADI_<KEY>.",
  );
  assert.match(codeHint("background", ""), /^Run as sh -c; keep it running/);
});

test("a webhook's relay settings have fields of their own and survive every save", () => {
  const relayed: Trigger = {
    ...base,
    runtime: "sh",
    code: "echo",
    enabled: true,
    extra: {
      relay: "public-eu",
      relay_endpoint: "ep_123",
      relay_retention: "3600",
      secret: "s",
      interval_secs: "5",
    },
  };
  // The free-form list never offers them: `relay_endpoint` is the machine's to write.
  assert.deepEqual(
    settingFields(draftOf(relayed), []).map((field) => field.key),
    ["secret", "interval_secs"],
  );
  assert.deepEqual(saveBody(draftOf(relayed), []).extra, {
    relay: "public-eu",
    relay_endpoint: "ep_123",
    relay_retention: "3600",
    secret: "s",
    interval_secs: "5",
  });
  // Clearing the relay clears it; the rest of the relay's settings are sent as they stand.
  assert.deepEqual(
    saveBody(
      { ...draftOf(relayed), extra: { ...relayed.extra!, relay: " ", relay_retention: "" } },
      [],
    ).extra,
    { relay_endpoint: "ep_123", secret: "s", interval_secs: "5" },
  );
  assert.deepEqual(enabledBody(relayed, false).extra, relayed.extra);
  assert.deepEqual(relayKeyKey("studio"), ["settings-relay-key", "studio"]);
});

test("a relay setting resolves to the address the sender is given", () => {
  assert.equal(resolveRelayUrl("public-eu"), "https://eu.webhooks.withadi.dev");
  assert.equal(resolveRelayUrl("public-us"), "https://us.webhooks.withadi.dev");
  assert.equal(resolveRelayUrl("enterprise-eu"), "https://eu.ent.webhooks.withadi.dev");
  assert.equal(resolveRelayUrl("enterprise-us"), "https://us.ent.webhooks.withadi.dev");
  assert.equal(resolveRelayUrl("custom: https://relay.example.com "), "https://relay.example.com");
  for (const none of ["", "custom", "custom:", "custom:  ", "elsewhere"])
    assert.equal(resolveRelayUrl(none), null, none);
  assert.equal(
    relayPublicUrl({ relay: "public-us", relay_endpoint: "ep_9" }),
    "https://us.webhooks.withadi.dev/h/ep_9",
  );
  assert.equal(
    relayPublicUrl({ relay: "custom:https://relay.example.com", relay_endpoint: "abc" }),
    "https://relay.example.com/h/abc",
  );
  // Not registered yet, or no relay at all: there is nothing to hand out.
  assert.equal(relayPublicUrl({ relay: "public-eu" }), null);
  assert.equal(relayPublicUrl({ relay_endpoint: "ep_9" }), null);
  assert.deepEqual(["", "public-eu", "custom", "custom:https://x"].map(relayChoice), [
    "",
    "public-eu",
    "custom",
    "custom",
  ]);
  assert.deepEqual(["custom", "custom:https://x", "public-eu"].map(isCustomRelay), [
    true,
    true,
    false,
  ]);
  assert.deepEqual(["custom:https://x", "custom", "public-eu"].map(customRelayUrl), [
    "https://x",
    "",
    "",
  ]);
});
