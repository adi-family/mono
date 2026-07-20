import { describe, it, expect } from "vitest";

import { resolveProvider, enabledProviders } from "../src/providers";
import type { Env } from "../src/types";

function env(overrides: Partial<Env> = {}): Env {
  return { STATE_SECRET: "s", APP_URL: "https://app.example", ...overrides } as Env;
}

describe("provider registry", () => {
  it("returns null for an unknown provider", () => {
    expect(resolveProvider("myspace", env())).toBeNull();
  });

  it("returns null when credentials are absent", () => {
    expect(resolveProvider("google", env())).toBeNull();
  });

  it("resolves with credentials and default scopes", () => {
    const p = resolveProvider(
      "google",
      env({ GOOGLE_CLIENT_ID: "id", GOOGLE_CLIENT_SECRET: "secret" }),
    );
    expect(p).not.toBeNull();
    expect(p!.clientId).toBe("id");
    expect(p!.clientSecret).toBe("secret");
    expect(p!.scopes).toEqual(["openid", "email", "profile"]);
    expect(p!.tokenUrl).toBe("https://oauth2.googleapis.com/token");
  });

  it("honours a scope override, split on spaces or commas", () => {
    const p = resolveProvider(
      "github",
      env({ GITHUB_CLIENT_ID: "id", GITHUB_CLIENT_SECRET: "secret", GITHUB_SCOPES: "repo, gist user" }),
    );
    expect(p!.scopes).toEqual(["repo", "gist", "user"]);
  });

  it("lists only the providers that are configured", () => {
    const e = env({ GOOGLE_CLIENT_ID: "id", GOOGLE_CLIENT_SECRET: "secret" });
    expect(enabledProviders(e)).toEqual(["google"]);
  });
});
