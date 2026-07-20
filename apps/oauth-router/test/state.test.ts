import { describe, it, expect } from "vitest";

import { signState, verifyState, randomNonce, type StatePayload } from "../src/state";

const SECRET = "test-secret";
const TTL = 600;

function payload(overrides: Partial<StatePayload> = {}): StatePayload {
  return { p: "google", r: "https://app.example/", n: "nonce123", t: 1_000_000, ...overrides };
}

describe("signed state", () => {
  it("round-trips a payload within the TTL", async () => {
    const token = await signState(payload(), SECRET);
    const back = await verifyState(token, SECRET, TTL, 1_000_000 + 30);
    expect(back).toEqual(payload());
  });

  it("rejects a wrong signing secret", async () => {
    const token = await signState(payload(), SECRET);
    expect(await verifyState(token, "other-secret", TTL, 1_000_000)).toBeNull();
  });

  it("rejects a tampered body", async () => {
    const token = await signState(payload(), SECRET);
    const [body, sig] = token.split(".");
    const tampered = `${body}x.${sig}`;
    expect(await verifyState(tampered, SECRET, TTL, 1_000_000)).toBeNull();
  });

  it("rejects an expired token", async () => {
    const token = await signState(payload({ t: 1_000_000 }), SECRET);
    expect(await verifyState(token, SECRET, TTL, 1_000_000 + TTL + 1)).toBeNull();
  });

  it("rejects a token stamped in the future beyond skew", async () => {
    const token = await signState(payload({ t: 2_000_000 }), SECRET);
    expect(await verifyState(token, SECRET, TTL, 1_000_000)).toBeNull();
  });

  it("rejects garbage", async () => {
    expect(await verifyState("not-a-token", SECRET, TTL, 1_000_000)).toBeNull();
    expect(await verifyState("", SECRET, TTL, 1_000_000)).toBeNull();
  });

  it("produces distinct url-safe nonces", () => {
    const a = randomNonce();
    const b = randomNonce();
    expect(a).not.toEqual(b);
    expect(a).toMatch(/^[A-Za-z0-9_-]+$/);
  });
});
