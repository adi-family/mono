/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  expiryText,
  filterSecrets,
  newSecretHref,
  oauthLoginUrl,
  oauthSecretBody,
  parkOAuth,
  parseOAuthReturn,
  parseScopeFilter,
  pendingOAuthKey,
  projectName,
  providerAccesses,
  providerLabel,
  scopeFilterValue,
  secretHref,
  secretId,
  secretNameProblem,
  secretRef,
  secretScopes,
  secretsKey,
  sortSecrets,
  takeOAuth,
  type Secret,
} from "./secrets";

const secret = (name: string, project?: string, extra: Partial<Secret> = {}): Secret => ({
  name,
  project: project ?? null,
  created_at: 1,
  updated_at: 2,
  ...extra,
});
const projects = [
  { id: "p-one", name: "One" },
  { id: "p-two", name: "Two" },
];
const list = [
  secret("OPENAI_API_KEY", undefined, { description: "Model access" }),
  secret("GMAIL_TOKEN", undefined, { oauth: { provider: "google", has_refresh: true } }),
  secret("API_KEY", "p-two", { description: "Billing sandbox" }),
  secret("API_KEY", "p-one"),
  secret("API_KEY"),
];

function memoryStorage(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    data,
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
    removeItem: (key: string) => void data.delete(key),
  };
}

test("secret reads are cached per machine, and a secret is its scope plus its name", () => {
  assert.deepEqual(secretsKey(), ["settings-secrets", ""]);
  assert.deepEqual(secretsKey("studio"), ["settings-secrets", "studio"]);
  assert.notEqual(secretId(secret("API_KEY")), secretId(secret("API_KEY", "p-one")));
  assert.equal(secretId({ name: "API_KEY", project: "" }), secretId(secret("API_KEY")));
  assert.deepEqual(secretRef(secret("API_KEY", "p-one")), { name: "API_KEY", project: "p-one" });
  assert.deepEqual(secretRef({ name: "API_KEY", project: "" }), { name: "API_KEY", project: null });
});

test("a secret's views keep its name and scope in the query", () => {
  assert.equal(secretHref(), "/settings/secrets");
  assert.equal(secretHref(secret("API_KEY")), "/settings/secrets?secret=API_KEY");
  assert.equal(
    secretHref(secret("API_KEY", "p-one")),
    "/settings/secrets?secret=API_KEY&project=p-one",
  );
  assert.equal(newSecretHref(), "/settings/secrets?new=1");
  assert.equal(newSecretHref("p-one"), "/settings/secrets?new=1&project=p-one");
});

test("a name must be an environment variable identifier", () => {
  for (const name of ["API_KEY", "DATABASE_URL", "token2", "_hidden", "X", "  PADDED  "])
    assert.equal(secretNameProblem(name), null, name);
  assert.equal(secretNameProblem("  "), "A secret name is required.");
  for (const name of ["2FA", "with space", "dash-ed", "dot.ted", "a/b", "ключ", "A=B"])
    assert.match(secretNameProblem(name) ?? "", /letters, digits and underscores/, name);
});

test("secrets list global first, then project by project, each by name", () => {
  assert.deepEqual(
    sortSecrets(list).map((item) => `${item.project ?? "global"}/${item.name}`),
    [
      "global/API_KEY",
      "global/GMAIL_TOKEN",
      "global/OPENAI_API_KEY",
      "p-one/API_KEY",
      "p-two/API_KEY",
    ],
  );
});

test("the scope filter counts what each scope holds and names projects", () => {
  assert.deepEqual(secretScopes(list, projects), [
    { value: "all", label: "All scopes", count: 5 },
    { value: "global", label: "Global", count: 3 },
    { value: "project:p-one", label: "One", count: 1 },
    { value: "project:p-two", label: "Two", count: 1 },
  ]);
  // A project that is archived, or not listed yet, is still a scope: it shows by its id.
  assert.equal(secretScopes([secret("X", "gone")])[2].label, "gone");
  assert.equal(projectName("p-one", projects), "One");
  for (const filter of ["all", "global", { project: "p-one" }] as const)
    assert.deepEqual(parseScopeFilter(scopeFilterValue(filter)), filter);
  assert.equal(parseScopeFilter("project:"), "all");
  assert.equal(parseScopeFilter("nonsense"), "all");
});

test("the list filter combines a scope with every search word", () => {
  const names = (items: Secret[]) =>
    items.map((item) => `${item.project ?? "global"}/${item.name}`);
  assert.equal(filterSecrets(list, "").length, 5);
  assert.deepEqual(names(filterSecrets(list, "", "global")), [
    "global/OPENAI_API_KEY",
    "global/GMAIL_TOKEN",
    "global/API_KEY",
  ]);
  assert.deepEqual(names(filterSecrets(list, "", { project: "p-one" })), ["p-one/API_KEY"]);
  assert.deepEqual(names(filterSecrets(list, "api_key two", "all", projects)), ["p-two/API_KEY"]);
  assert.deepEqual(names(filterSecrets(list, "google")), ["global/GMAIL_TOKEN"]);
  assert.deepEqual(names(filterSecrets(list, "MODEL access")), ["global/OPENAI_API_KEY"]);
  // "global" is searchable too: a substring match, so OPENAI_API_KEY is an api_key as well.
  assert.deepEqual(names(filterSecrets(list, "global api_key")), [
    "global/OPENAI_API_KEY",
    "global/API_KEY",
  ]);
  assert.deepEqual(filterSecrets(list, "billing", "global"), []);
});

test("an OAuth secret shows its provider and a coarse expiry", () => {
  assert.deepEqual(["google", "github", "linear", ""].map(providerLabel), [
    "Google",
    "GitHub",
    "Linear",
    "OAuth",
  ]);
  const now = 1_800_000_000_000;
  const at = (seconds: number) => expiryText(now / 1000 + seconds, now);
  assert.deepEqual([-5, 0, 59, 60, 3599, 3600, 47 * 3600, 48 * 3600, 30 * 86400].map(at), [
    "expired",
    "expired",
    "expires in 0m",
    "expires in 1m",
    "expires in 59m",
    "expires in 1h",
    "expires in 47h",
    "expires in 2d",
    "expires in 30d",
  ]);
  assert.equal(expiryText(null), null);
  assert.equal(expiryText(undefined), null);
  assert.equal(providerAccesses("google").length, 5);
  assert.deepEqual(providerAccesses("github"), []);
});

test("the sign-in address carries the return address and the requested access", () => {
  assert.equal(
    oauthLoginUrl(
      "google",
      "http://app.adi/settings/secrets",
      "email https://mail.google.com/",
      "https://router.test",
    ),
    "https://router.test/login/google?redirect=http%3A%2F%2Fapp.adi%2Fsettings%2Fsecrets&scope=email+https%3A%2F%2Fmail.google.com%2F",
  );
  assert.equal(
    oauthLoginUrl("github", "http://app.adi/settings/secrets", " ", "https://router.test"),
    "https://router.test/login/github?redirect=http%3A%2F%2Fapp.adi%2Fsettings%2Fsecrets",
  );
});

test("a parked sign-in is read once, and only if it is well formed", () => {
  const storage = memoryStorage();
  parkOAuth(
    {
      name: "GMAIL_TOKEN",
      project: "p-one",
      description: "Inbox",
      provider: "google",
      source: "studio",
    },
    storage,
  );
  assert.deepEqual(takeOAuth(storage), {
    name: "GMAIL_TOKEN",
    project: "p-one",
    description: "Inbox",
    provider: "google",
    source: "studio",
  });
  assert.equal(takeOAuth(storage), null, "consumed by the first read");
  // The old panel parks the same shape without a machine.
  assert.deepEqual(
    takeOAuth(
      memoryStorage({ [pendingOAuthKey]: JSON.stringify({ name: "T", provider: "github" }) }),
    ),
    { name: "T", project: null, description: null, provider: "github" },
  );
  for (const bad of [
    "not json",
    "null",
    JSON.stringify({ name: "bad name", provider: "google" }),
    JSON.stringify({ name: "T" }),
    JSON.stringify({ name: "T", provider: "google", source: "Not/A/Node" }),
  ]) {
    const store = memoryStorage({ [pendingOAuthKey]: bad });
    const taken = takeOAuth(store);
    assert.equal(store.data.has(pendingOAuthKey), false, "a malformed intent is still removed");
    if (bad.includes("Not/A/Node"))
      assert.deepEqual(
        taken,
        { name: "T", project: null, description: null, provider: "google" },
        "an unusable machine falls back to this one",
      );
    else assert.equal(taken, null, bad);
  }
});

test("a returning sign-in is a token, a provider error, or nothing to do with OAuth", () => {
  assert.deepEqual(parseOAuthReturn(""), { kind: "none" });
  assert.deepEqual(parseOAuthReturn("#trigger-form"), { kind: "none" });
  assert.deepEqual(parseOAuthReturn("#error=access_denied&error_description=The+user+said+no"), {
    kind: "error",
    message: "The user said no",
  });
  assert.deepEqual(parseOAuthReturn("#error=access_denied"), {
    kind: "error",
    message: "access_denied",
  });
  const token = parseOAuthReturn(
    "#access_token=at-1&refresh_token=rt-1&expires_in=3599&scope=email+openid&provider=google",
  );
  assert.deepEqual(token, {
    kind: "token",
    accessToken: "at-1",
    refreshToken: "rt-1",
    expiresIn: 3599,
    scope: "email openid",
    provider: "google",
  });
  assert.deepEqual(parseOAuthReturn("#access_token=at-2&expires_in=soon"), {
    kind: "token",
    accessToken: "at-2",
    refreshToken: null,
    expiresIn: null,
    scope: null,
    provider: null,
  });
  assert.equal(token.kind, "token");
  if (token.kind !== "token") return;
  assert.deepEqual(
    oauthSecretBody(
      { name: "GMAIL_TOKEN", project: null, description: "Inbox", provider: "github" },
      token,
    ),
    {
      name: "GMAIL_TOKEN",
      project: null,
      description: "Inbox",
      provider: "google",
      access_token: "at-1",
      refresh_token: "rt-1",
      expires_in: 3599,
      scope: "email openid",
    },
  );
  // The provider named when the sign-in started stands when the router does not name one.
  assert.equal(
    oauthSecretBody(
      { name: "T", project: "p", description: null, provider: "github" },
      { ...token, provider: null },
    ).provider,
    "github",
  );
});
