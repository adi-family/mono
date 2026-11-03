/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";

import { legacyChatDestination } from "./primary-routes";

test("local chat bookmarks preserve encoded agent and run identities", () => {
  const href = legacyChatDestination("/chat/reviewer%20%26%20writer/run%2Fone%3Fx%3Dy");
  const url = new URL(href!, "http://app.adi");
  assert.equal(url.pathname, "/interface");
  assert.equal(url.searchParams.get("agent"), "reviewer & writer");
  assert.equal(url.searchParams.get("run"), "run/one?x=y");
  assert.equal(url.searchParams.has("node"), false);
});

test("remote bookmarks keep their paired machine and never become local chats", () => {
  const href = legacyChatDestination("/chat/@studio/designer/run-one/");
  const params = new URL(href!, "http://app.adi").searchParams;
  assert.equal(params.get("node"), "studio");
  assert.equal(params.get("agent"), "designer");
  assert.equal(params.get("run"), "run-one");
});

test("agent-only bookmarks retain their agent and machine without selecting a run", () => {
  for (const [path, node] of [["/chat/reviewer%20%26%20writer", null], ["/chat/@studio/reviewer%20%26%20writer/", "studio"]] as const) {
    const params = new URL(legacyChatDestination(path)!, "http://app.adi").searchParams;
    assert.equal(params.get("agent"), "reviewer & writer");
    assert.equal(params.get("node"), node);
    assert.equal(params.has("run"), true);
    assert.equal(params.get("run"), "");
  }
});

test("unselected chat bookmarks retain their machine without auto-resuming history", () => {
  for (const [path, node] of [["/chat", null], ["/chat/", null], ["/chat/@studio", "studio"], ["/chat/@studio/", "studio"]] as const) {
    const params = new URL(legacyChatDestination(path)!, "http://app.adi").searchParams;
    assert.equal(params.has("agent"), false);
    assert.equal(params.get("node"), node);
    assert.equal(params.has("run"), true);
    assert.equal(params.get("run"), "");
  }
});

test("malformed and unrelated bookmarks cannot launch or select a different chat", () => {
  for (const path of ["/extended/settings", "/chatty", "/chat/a//b", "/chat/@", "/chat/@../a/b", "/chat/@Studio/a/b", "/chat/%E0%A4%A/b", "/chat/@studio/a/b/extra", "/chat/a/b/extra"]) {
    assert.equal(legacyChatDestination(path), null, path);
  }
});
