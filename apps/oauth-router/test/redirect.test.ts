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

  it("matches whole origins — scheme, host and port all count", () => {
    // The two halves of the check disagree on purpose, and this is the seam that bites twice:
    // the http exemption looks only at the *hostname*, while the allow-list matches whole
    // *origins*. So the same app is refused when reached over the other scheme or on another
    // port, which reads in production as "redirect target is missing or not allow-listed".
    // Both halves of this were live bugs: ADI's front door answers on http *and* https for
    // app.adi and the browser upgrades to https on its own, and adi-app is reachable directly
    // on several loopback ports as well as through the front door.
    const e = env({ APP_URL: "http://app.adi", ALLOWED_REDIRECT_ORIGINS: "http://localhost:8000" });
    expect(allowedRedirect("http://app.adi/extended/secrets", e)).toBe(
      "http://app.adi/extended/secrets",
    );
    // Same host, other scheme. Note it clears the transport gate — https always does — and
    // still fails, because the allow-list holds http://app.adi and not https://app.adi.
    expect(allowedRedirect("https://app.adi/extended/secrets", e)).toBeNull();

    expect(allowedRedirect("http://localhost:8000/extended/secrets", e)).toBe(
      "http://localhost:8000/extended/secrets",
    );
    // Same host, other port; and same port, other spelling of loopback. Distinct origins both.
    expect(allowedRedirect("http://localhost:8090/extended/secrets", e)).toBeNull();
    expect(allowedRedirect("http://127.0.0.1:8000/extended/secrets", e)).toBeNull();
  });

  it("rejects a host that merely ends in the allowed one", () => {
    // `app.adi.evil.example` does not end in `.adi`, so it never reaches the origin check —
    // but it is the shape of a lookalike worth having pinned.
    const e = env({ APP_URL: "http://app.adi" });
    expect(allowedRedirect("http://app.adi.evil.example/x", e)).toBeNull();
    expect(allowedRedirect("https://app.adi.evil.example/x", e)).toBeNull();
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
