import { describe, it, expect } from "vitest";

import { allowedRedirect, deliveryUrl } from "../src/redirect";
import type { Env } from "../src/types";

function env(overrides: Partial<Env> = {}): Env {
  return { STATE_SECRET: "s", APP_URL: "https://app.example", ...overrides } as Env;
}

describe("allowedRedirect", () => {
  it("falls back to APP_URL when no candidate is given", () => {
    expect(allowedRedirect(null, env())).toBe("https://app.example");
  });

  it("accepts a candidate on APP_URL's origin", () => {
    expect(allowedRedirect("https://app.example/welcome", env())).toBe("https://app.example/welcome");
  });

  it("rejects a candidate on a foreign origin", () => {
    expect(allowedRedirect("https://evil.example/steal", env())).toBeNull();
  });

  it("accepts extra allow-listed origins", () => {
    const e = env({ ALLOWED_REDIRECT_ORIGINS: "https://other.example, http://localhost:8000" });
    expect(allowedRedirect("https://other.example/x", e)).toBe("https://other.example/x");
    expect(allowedRedirect("http://localhost:8000/x", e)).toBe("http://localhost:8000/x");
  });

  it("rejects a non-https, non-local target", () => {
    const e = env({ ALLOWED_REDIRECT_ORIGINS: "http://plain.example" });
    expect(allowedRedirect("http://plain.example/x", e)).toBeNull();
  });

  it("accepts http on the local .adi zone, any path, when allow-listed", () => {
    const e = env({ APP_URL: "http://app.adi" });
    expect(allowedRedirect("http://app.adi/oauth/callback/some", e)).toBe(
      "http://app.adi/oauth/callback/some",
    );
    expect(allowedRedirect(null, e)).toBe("http://app.adi");
  });

  it("still gates a .adi host through the allow-list", () => {
    // http is permitted for .adi, but http://app.adi is not on this deployment's list.
    expect(allowedRedirect("http://app.adi/x", env())).toBeNull();
  });

  it("rejects an unparsable candidate", () => {
    expect(allowedRedirect("::::not a url", env())).toBeNull();
  });
});

describe("deliveryUrl", () => {
  it("puts params in the fragment and drops empties", () => {
    const url = deliveryUrl("https://app.example/", {
      provider: "google",
      access_token: "abc",
      expires_in: "3600",
      scope: undefined,
      id_token: "",
    });
    const u = new URL(url);
    expect(u.origin + u.pathname).toBe("https://app.example/");
    const frag = new URLSearchParams(u.hash.slice(1));
    expect(frag.get("provider")).toBe("google");
    expect(frag.get("access_token")).toBe("abc");
    expect(frag.get("expires_in")).toBe("3600");
    expect(frag.has("scope")).toBe(false);
    expect(frag.has("id_token")).toBe(false);
  });

  it("replaces any pre-existing fragment", () => {
    const url = deliveryUrl("https://app.example/#stale", { access_token: "abc" });
    expect(new URL(url).hash).toBe("#access_token=abc");
  });
});
