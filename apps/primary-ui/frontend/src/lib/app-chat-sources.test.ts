/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";
import { readLastLaunchedAgent, conversationKey, hasSource, normalizeSources, routeNode, sameConversation, selectedSources } from "./app-chat-sources";
import { appConversationHref, conversationHref } from "./app-chat-transcript";

test("source preferences restore a nonempty sorted selection and reject malformed storage", () => {
  assert.deepEqual(normalizeSources({ local: false, nodes: ["zomro", "hetzner", "zomro"] }), { local: false, nodes: ["hetzner", "zomro"] });
  assert.deepEqual(normalizeSources({ local: false, nodes: [] }), { local: true, nodes: [] });
  for (const invalid of [null, [], { local: "false", nodes: [] }, { nodes: ["../local"] }, { nodes: "node" }]) assert.deepEqual(normalizeSources(invalid), { local: true, nodes: [] });
  assert.deepEqual(selectedSources({ local: true, nodes: ["local"] }), [undefined, "local"]);
  assert.equal(hasSource({ local: false, nodes: ["local"] }, undefined), false);
  assert.equal(hasSource({ local: false, nodes: ["local"] }, "local"), true);
});

test("identical run identifiers remain distinct across owning machines", () => {
  const local = { agent: "designer", runId: "same-run" };
  const remote = { ...local, source: "local" };
  assert.notEqual(conversationKey(local), conversationKey(remote));
  assert.equal(sameConversation(local, remote), false);
  assert.equal(sameConversation(remote, { ...remote }), true);
  assert.equal(sameConversation(local, null), false);
});

test("conversation and main app links preserve source and encode agent/run names", () => {
  const remote = { agent: "reviewer & writer", runId: "run/one?x=y", source: "hetzner" };
  assert.equal(conversationHref(remote), "/interface?agent=reviewer%20%26%20writer&run=run%2Fone%3Fx%3Dy&node=hetzner");
  assert.equal(appConversationHref(remote), "http://app.adi/chat/@hetzner/reviewer%20%26%20writer/run%2Fone%3Fx%3Dy");
  assert.equal(appConversationHref({ agent: "designer", runId: "one" }), "http://app.adi/chat/designer/one");
  assert.equal(routeNode("hetzner-adi"), "hetzner-adi");
  for (const value of [null, "", "../node", "node.local", "NODE", "node/"]) assert.equal(routeNode(value), undefined);
});

test("last launched agent is browser-local, validates the owning machine and tolerates unavailable storage", () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  try {
    for (const [stored, expected] of [
      ['{"agent":"designer"}', {agent:"designer"}],
      ['{"agent":"designer","source":"laptop-b"}', {agent:"designer",source:"laptop-b"}],
      ['{"agent":"designer","source":"../node"}', undefined],
      ['{"agent":" "}', undefined], ['null', undefined], ['broken', undefined],
    ] as const) {
      Object.defineProperty(globalThis, "localStorage", {configurable:true,value:{getItem:()=>stored}});
      assert.deepEqual(readLastLaunchedAgent(), expected);
    }
    Object.defineProperty(globalThis, "localStorage", {configurable:true,get:()=>{throw new Error("Blocked");}});
    assert.equal(readLastLaunchedAgent(), undefined);
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "localStorage", descriptor);
    else Reflect.deleteProperty(globalThis, "localStorage");
  }
});
