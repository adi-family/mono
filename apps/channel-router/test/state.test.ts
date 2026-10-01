import { describe, it, expect } from "vitest";

import {
  signLinkCode,
  verifyLinkCode,
  signNodeToken,
  verifyNodeToken,
  randomId,
  LINK_CODE_TTL_SECONDS,
  type LinkCodePayload,
  type NodeTokenPayload,
} from "../src/state";

const SECRET = "test-secret";

function linkPayload(overrides: Partial<LinkCodePayload> = {}): LinkCodePayload {
  return { node: "node-1", provider: "telegram", connection: "conn-1", t: 1_000_000, ...overrides };
}

function tokenPayload(overrides: Partial<NodeTokenPayload> = {}): NodeTokenPayload {
  return { node: "node-1", provider: "telegram", epoch: 1, t: 1_000_000, ...overrides };
}

describe("link code", () => {
  it("round-trips within the TTL", async () => {
    const code = await signLinkCode(linkPayload(), SECRET);
    const back = await verifyLinkCode(code, SECRET, 1_000_000 + 30);
    expect(back).toEqual(linkPayload());
  });

  it("rejects a wrong secret", async () => {
    const code = await signLinkCode(linkPayload(), SECRET);
    expect(await verifyLinkCode(code, "other-secret", 1_000_000)).toBeNull();
  });

  it("rejects a tampered body", async () => {
    const code = await signLinkCode(linkPayload(), SECRET);
    const [body, sig] = code.split(".");
    expect(await verifyLinkCode(`${body}x.${sig}`, SECRET, 1_000_000)).toBeNull();
  });

  it("rejects an expired code", async () => {
    const code = await signLinkCode(linkPayload({ t: 1_000_000 }), SECRET);
    expect(await verifyLinkCode(code, SECRET, 1_000_000 + LINK_CODE_TTL_SECONDS + 1)).toBeNull();
  });

  it("rejects garbage", async () => {
    expect(await verifyLinkCode("not-a-token", SECRET, 1_000_000)).toBeNull();
    expect(await verifyLinkCode("", SECRET, 1_000_000)).toBeNull();
  });
});

describe("node token", () => {
  it("round-trips", async () => {
    const token = await signNodeToken(tokenPayload(), SECRET);
    const back = await verifyNodeToken(token, SECRET, 1_000_000 + 30);
    expect(back).toEqual(tokenPayload());
  });

  it("rejects a wrong secret", async () => {
    const token = await signNodeToken(tokenPayload(), SECRET);
    expect(await verifyNodeToken(token, "other-secret", 1_000_000)).toBeNull();
  });

  it("survives a long gap -- it does not rotate on reconnect", async () => {
    const token = await signNodeToken(tokenPayload({ t: 1_000_000 }), SECRET);
    // A month later, same token, still good -- §2: "the token does not rotate on reconnect."
    expect(await verifyNodeToken(token, SECRET, 1_000_000 + 30 * 24 * 60 * 60)).not.toBeNull();
  });
});

describe("randomId", () => {
  it("produces distinct url-safe ids", () => {
    const a = randomId();
    const b = randomId();
    expect(a).not.toEqual(b);
    expect(a).toMatch(/^[A-Za-z0-9_-]+$/);
  });
});
