/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";

import { APP_API_BASE, IS_PRIMARY_APP, appApiBase, appHref } from "./app-api";
import { appConversationHref, conversationHref } from "./app-chat-transcript";
import { attachmentUrl, getAgents, getFleetNodes, getVoice, transcribeAudio, uploadImage } from "./app-chat";
import { getFleetMachines, getLocalApps, lockMachine, unlockMachine } from "./home-apps";

// Run once without overrides and once with VITE_APP_API_BASE=/api to exercise both hosts.
const expectedBase = process.env.VITE_APP_API_BASE?.replace(/\/+$/, "") || "/api/app";

test("the configured API base selects native or showcase requests", () => {
  assert.equal(APP_API_BASE, expectedBase);
  assert.equal(appApiBase(), expectedBase);
  assert.equal(IS_PRIMARY_APP, expectedBase === "/api");
  assert.equal(appApiBase("laptop-b"), `${expectedBase}/node/laptop-b/api`);
  for (const source of ["", "../local", "a/b", "@laptop", "laptop.example", "Laptop"]) {
    assert.throws(() => appApiBase(source), /valid paired machine/);
  }
});

test("settings and conversation links stay on the primary panel's origin", () => {
  assert.equal(appHref("/extended/settings"), `${expectedBase === "/api" ? "" : "http://app.adi"}/extended/settings`);
  assert.equal(appHref("/extended/settings/fleet"), `${expectedBase === "/api" ? "" : "http://app.adi"}/extended/settings/fleet`);
  const remote = { agent: "reviewer & writer", runId: "run/one?x=y", source: "laptop-b" };
  assert.equal(appConversationHref(remote), expectedBase === "/api"
    ? conversationHref(remote)
    : "http://app.adi/chat/@laptop-b/reviewer%20%26%20writer/run%2Fone%3Fx%3Dy");
});

test("catalog, access, chat, image, and voice entry points share the configured API", async (context) => {
  const originalFetch = globalThis.fetch;
  context.after(() => { globalThis.fetch = originalFetch; });
  const calls: string[] = [];
  globalThis.fetch = (async (input) => {
    calls.push(String(input));
    return Response.json({ ok: true, agents: [], nodes: [], dashboards: [], engines: [], default_engine: "browser", text: "Recorded text", id: "image-one" });
  }) as typeof fetch;

  await getAgents();
  await getAgents(undefined, "laptop-b");
  await getFleetNodes();
  await getLocalApps();
  await getFleetMachines();
  await unlockMachine("laptop-b", "adi", "fixture-password");
  await lockMachine("laptop-b");
  await getVoice();
  await transcribeAudio(new Blob(["fixture"], { type: "audio/webm" }), "openai");
  await uploadImage(new File(["fixture"], "image.png", { type: "image/png" }), "laptop-b");

  assert.deepEqual(calls, [
    "/agents", "/node/laptop-b/api/agents", "/fleet/nodes", "/dashboards", "/fleet/dashboards",
    "/fleet/dashboards/unlock", "/fleet/dashboards/forget", "/voice", "/voice/transcribe?engine=openai",
    "/node/laptop-b/api/agents/attachment",
  ].map((path) => `${expectedBase}${path}`));
  assert.equal(attachmentUrl("image/one", "laptop-b"), `${expectedBase}/node/laptop-b/api/agents/attachment/image%2Fone`);
});
