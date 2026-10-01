import { describe, it, expect } from "vitest";

import { enabledProviders, PROVIDERS } from "../src/providers";
import type { Env } from "../src/types";

function env(overrides: Partial<Env> = {}): Env {
  return {
    ROUTER_SECRET: "s",
    ROUTER_ADMIN_SECRET: "a",
    NODE_CONNECTION: undefined as never,
    ROUTING_KEYS: undefined as never,
    ...overrides,
  };
}

describe("provider registry", () => {
  it("lists telegram and slack", () => {
    expect(Object.keys(PROVIDERS)).toEqual(["telegram", "slack"]);
  });

  it("telegram is enabled once both its credentials are set", () => {
    expect(enabledProviders(env())).toEqual([]);
    expect(
      enabledProviders(env({ TELEGRAM_BOT_TOKEN: "t", TELEGRAM_SECRET_TOKEN: "s" })),
    ).toEqual(["telegram"]);
  });

  it("telegram stays disabled with only one of its two credentials", () => {
    expect(enabledProviders(env({ TELEGRAM_BOT_TOKEN: "t" }))).toEqual([]);
    expect(enabledProviders(env({ TELEGRAM_SECRET_TOKEN: "s" }))).toEqual([]);
  });

  it("slack is never enabled -- stub only, no adapter yet", () => {
    expect(
      enabledProviders(
        env({
          TELEGRAM_BOT_TOKEN: "t",
          TELEGRAM_SECRET_TOKEN: "s",
          SLACK_SIGNING_SECRET: "whatever",
        } as Partial<Env> as Env),
      ),
    ).toEqual(["telegram"]);
  });
});
