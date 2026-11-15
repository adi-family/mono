import { useEffect, useRef, useState, type ReactNode } from "react";
import { useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { ArrowLeft, Copy, Eye, EyeOff, KeyRound, LoaderCircle, Plus, RotateCw, Search, Trash2, X } from "lucide-react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";

import { Badge, Button, Card, Select, SettingsSection } from "@design-system/ui";

import { IS_PRIMARY_APP } from "../../lib/app-api";
import { sourceName } from "../../lib/app-chat-sources";
import { copyText } from "../../lib/copy-text";
import {
  expiryText, filterSecrets, loadSecrets, newSecretHref, oauthLoginUrl, oauthSecretBody, parkOAuth, parseOAuthReturn, parseScopeFilter,
  projectName, providerLabel, scopeFilterValue, secretHref, secretId, secretRef, secretScopes, secretsKey, sortSecrets, takeOAuth,
  type PendingOAuth, type RevealedSecret, type ScopeFilter, type Secret, type SecretsState, type SetSecret,
} from "../../lib/secrets";
import { loadSettingsProjects, settingsProjectsKey, settingsRequest, type SettingsProject } from "../../lib/settings-api";
import { useSettingsSource } from "../../lib/settings-source";
import { formatAge } from "../../lib/triggers";
import { ConfirmDialog } from "./ConfirmDialog";
import { NewSecretForm, ReplaceSecretForm } from "./SecretEditor";

const description = "Encrypted key-values this machine injects into runs as environment variables. The list shows names only; a value leaves the machine when you reveal or copy it.";
/** A revealed value is put away again on its own. */
const revealMs = 60_000;

const fullDate = (unixSeconds: number) => new Date(unixSeconds * 1000).toLocaleString();

/**
 * Every write answers with the machine's fresh secret list, which replaces the cached one. A
 * revealed value is never cached: it lives in this hook's state, per secret, and is dropped
 * after a minute, when hidden, when the secret changes, and when the section closes.
 */
function useSecretActions(source?: string) {
  const cache = useQueryClient();
  const pending = useRef(false);
  const carried = useRef(false);
  const timers = useRef(new Map<string, ReturnType<typeof setTimeout>>());
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Which action the error came from, so a refused save is reported beside its own form.
  const [failed, setFailed] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [revealed, setRevealed] = useState<Record<string, string>>({});

  function hide(id: string) {
    const timer = timers.current.get(id);
    if (timer) clearTimeout(timer);
    timers.current.delete(id);
    setRevealed((current) => (id in current ? Object.fromEntries(Object.entries(current).filter(([key]) => key !== id)) : current));
  }
  useEffect(() => () => { for (const timer of timers.current.values()) clearTimeout(timer); }, []);

  async function run<T>(label: string, work: () => Promise<T>): Promise<T | null> {
    if (pending.current) return null;
    pending.current = true;
    setBusy(label); setError(null); setFailed(null); setNotice(null);
    try { return await work(); }
    catch (cause) { setError(cause instanceof Error ? cause.message : "The request failed. Try again."); setFailed(label); return null; }
    finally { pending.current = false; setBusy(null); }
  }
  /** A write on `machine` (this page's, unless a sign-in was started for another). */
  async function write(label: string, path: string, body: unknown, machine = source): Promise<SecretsState | null> {
    return run(label, async () => {
      const state = await settingsRequest<SecretsState>(machine, path, body);
      await cache.cancelQueries({ queryKey: secretsKey(machine) });
      cache.setQueryData(secretsKey(machine), state);
      return state;
    });
  }
  const fetchValue = (secret: Secret) => run(`reveal:${secretId(secret)}`, async () => (await settingsRequest<RevealedSecret>(source, "/secrets/reveal", secretRef(secret))).value);

  return {
    busy, error, notice, revealed, hide,
    /** The error, when it came from saving a value: the form that sent it shows it. */
    saveError: failed?.startsWith("set:") ? error : null,
    announce: (message: string, carry = false) => { carried.current = carry; setNotice(message); },
    fail: (message: string, carry = false) => { carried.current = carry; setFailed(null); setError(message); },
    clear: () => { setError(null); setFailed(null); setNotice(null); },
    /** The page moved to another view: a result message belongs to the view it was shown in. */
    moved: () => {
      if (carried.current) { carried.current = false; return; }
      setError(null); setFailed(null); setNotice(null);
    },
    set: async (body: SetSecret) => { const state = await write(`set:${secretId(body)}`, "/secrets/set", body); if (state) hide(secretId(body)); return state; },
    setOAuth: (machine: string | undefined, body: ReturnType<typeof oauthSecretBody>) => write(`set:${secretId(body)}`, "/secrets/set-oauth", body, machine),
    remove: async (secret: Secret) => { const state = await write(`remove:${secretId(secret)}`, "/secrets/remove", secretRef(secret)); if (state) hide(secretId(secret)); return state; },
    refresh: async (secret: Secret) => { const state = await write(`refresh:${secretId(secret)}`, "/secrets/refresh", secretRef(secret)); if (state) hide(secretId(secret)); return state; },
    reveal: async (secret: Secret) => {
      const id = secretId(secret);
      const value = await fetchValue(secret);
      if (value === null) return false;
      setRevealed((current) => ({ ...current, [id]: value }));
      const previous = timers.current.get(id);
      if (previous) clearTimeout(previous);
      timers.current.set(id, setTimeout(() => hide(id), revealMs));
      return true;
    },
    /** Put the value on the clipboard without showing it. `null` when it could not be read. */
    copy: async (secret: Secret) => {
      const value = await fetchValue(secret);
      return value === null ? null : copyText(value);
    },
  };
}
type SecretActions = ReturnType<typeof useSecretActions>;

function ScopeBadge({ secret, projects }: { secret: Secret; projects: SettingsProject[] }) {
  return <Badge data-slot="secret-scope" variant={secret.project ? "outline" : "secondary"} title={secret.project ? `Project ${secret.project}` : "Available to every run"}>{secret.project ? projectName(secret.project, projects) : "Global"}</Badge>;
}

function OAuthBadge({ secret }: { secret: Secret }) {
  if (!secret.oauth) return null;
  const expiry = expiryText(secret.oauth.expires_at);
  return <Badge data-slot="secret-oauth" variant={expiry === "expired" ? "warning" : "muted"} dot={expiry === "expired"} title="Obtained through an OAuth sign-in">{providerLabel(secret.oauth.provider)}{expiry ? ` · ${expiry}` : ""}</Badge>;
}

function RevealedValue({ value }: { value: string }) {
  return <pre data-slot="secret-value" tabIndex={0} aria-label="Revealed value" className="bg-muted/50 focus-visible:ring-ring mt-2.5 max-h-60 overflow-auto rounded-md border p-2.5 font-mono text-xs leading-relaxed break-all whitespace-pre-wrap outline-none focus-visible:ring-2">{value || " "}</pre>;
}

/** Reveal or hide, and copy. Only in the ADI app itself: the showcase relay never carries a value out. */
function ValueActions({ secret, actions }: { secret: Secret; actions: SecretActions }) {
  const id = secretId(secret);
  const shown = id in actions.revealed;
  const working = actions.busy === `reveal:${id}`;
  if (!IS_PRIMARY_APP) return null;
  return (
    <>
      <Button variant="ghost" size="sm" data-slot="secret-reveal" aria-pressed={shown} aria-label={`${shown ? "Hide" : "Reveal"} ${secret.name}`} onClick={() => { if (shown) actions.hide(id); else void actions.reveal(secret); }}>
        {working ? <LoaderCircle className="size-3.5 motion-safe:animate-spin" aria-hidden="true" /> : shown ? <EyeOff className="size-3.5" aria-hidden="true" /> : <Eye className="size-3.5" aria-hidden="true" />}
        {shown ? "Hide" : "Reveal"}
      </Button>
      <Button variant="ghost" size="sm" data-slot="secret-copy" aria-label={`Copy ${secret.name}`} onClick={() => void actions.copy(secret).then((copied) => {
        if (copied === null) return;
        if (copied) actions.announce(`Copied ${secret.name} to the clipboard.`);
        else actions.fail(`Couldn’t reach the clipboard. Reveal ${secret.name} and copy it by hand.`);
      })}>
        <Copy className="size-3.5" aria-hidden="true" />
        Copy
      </Button>
    </>
  );
}

function LoadProblem({ machine, query }: { machine: string; query: UseQueryResult<SecretsState> }) {
  return (
    <div role="alert" className="rounded-xl border p-6">
      <p>Couldn’t load the secrets on {machine}.</p>
      <p className="text-muted-foreground mt-1 text-sm">{query.error instanceof Error ? query.error.message : "The request failed."}</p>
      <Button variant="outline" className="mt-4" onClick={() => void query.refetch()}>Try again</Button>
    </div>
  );
}

/** The result of the last action. A refused save is left to the form that sent it. */
function Messages({ actions, quiet = false }: { actions: SecretActions; quiet?: boolean }) {
  const error = quiet || actions.saveError ? null : actions.error;
  return (
    <>
      {actions.notice && <p role="status" data-slot="secret-notice" className="bg-muted mb-4 rounded-lg border p-3 text-sm">{actions.notice}</p>}
      {error && <p role="alert" data-slot="secret-error" className="text-destructive mb-4 rounded-lg border p-3 text-sm">{error}</p>}
    </>
  );
}

function SecretRow({ secret, projects, actions }: { secret: Secret; projects: SettingsProject[]; actions: SecretActions }) {
  const value = actions.revealed[secretId(secret)];
  return (
    <li data-slot="secret-row" data-secret={secret.name} data-scope={secret.project ?? ""} className="px-4 py-3">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <div className="min-w-0 flex-1 basis-64">
          <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1.5">
            <Link to={secretHref(secret)} className="focus-visible:ring-ring rounded-sm font-mono text-sm font-medium break-all underline-offset-4 outline-none hover:underline focus-visible:ring-2">{secret.name}</Link>
            <ScopeBadge secret={secret} projects={projects} />
            <OAuthBadge secret={secret} />
          </div>
          {secret.description && <p className="text-muted-foreground mt-1 line-clamp-2 text-sm leading-relaxed" title={secret.description}>{secret.description}</p>}
        </div>
        <div className="flex shrink-0 items-center gap-1 max-sm:-ml-2.5 max-sm:w-[calc(100%_+_0.625rem)]">
          <span className="text-muted-foreground mr-2 text-xs max-sm:order-last max-sm:mr-0 max-sm:ml-auto" title={`Updated ${fullDate(secret.updated_at)}`}>Updated {formatAge(secret.updated_at)}</span>
          <ValueActions secret={secret} actions={actions} />
        </div>
      </div>
      {value !== undefined && <RevealedValue value={value} />}
    </li>
  );
}

function SecretList({ machine, query, projects, actions }: { machine: string; query: UseQueryResult<SecretsState>; projects: SettingsProject[]; actions: SecretActions }) {
  const [search, setSearch] = useState("");
  const [scope, setScope] = useState<ScopeFilter>("all");
  const secrets = sortSecrets(query.data?.secrets ?? []);
  const scopes = secretScopes(secrets, projects);
  // A scope whose last secret was removed is no longer offered: fall back to all of them.
  const activeScope = scopes.some((item) => item.value === scopeFilterValue(scope)) ? scope : "all";
  const shown = filterSecrets(secrets, search, activeScope, projects);
  const filtered = !!search.trim() || activeScope !== "all";

  return (
    <SettingsSection
      data-slot="settings-secrets"
      title="Secrets"
      description={description}
      actions={<Button asChild data-slot="secret-new"><Link to={newSecretHref(typeof activeScope === "object" ? activeScope.project : null)}><Plus className="size-4" aria-hidden="true" />New secret</Link></Button>}
    >
      <Messages actions={actions} />
      {query.isPending ? (
        <p role="status" className="text-muted-foreground flex items-center gap-2 py-12 text-sm"><LoaderCircle className="size-4 motion-safe:animate-spin" aria-hidden="true" /> Loading secrets…</p>
      ) : query.isError && !query.data ? (
        <LoadProblem machine={machine} query={query} />
      ) : secrets.length === 0 ? (
        <div data-slot="secrets-empty" className="rounded-xl border border-dashed px-6 py-14 text-center">
          <KeyRound className="text-muted-foreground mx-auto mb-4 size-6" aria-hidden="true" />
          <h4 className="font-medium">No secrets on {machine} yet</h4>
          <p className="text-muted-foreground mx-auto mt-2 max-w-sm text-sm leading-relaxed">Store an API key or a token once, and every run that is allowed it finds it in its environment.</p>
          <Button asChild className="mt-5"><Link to={newSecretHref()}><Plus className="size-4" aria-hidden="true" />New secret</Link></Button>
        </div>
      ) : (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-3">
            <div className="focus-within:ring-ring flex min-w-48 flex-1 items-center gap-2 rounded-lg border px-3 focus-within:ring-2 sm:max-w-72">
              <Search className="text-muted-foreground size-4 shrink-0" aria-hidden="true" />
              <input aria-label="Search secrets" placeholder="Search secrets…" value={search} onChange={(event) => setSearch(event.target.value)} className="placeholder:text-muted-foreground h-8 w-full min-w-0 bg-transparent text-sm outline-none" />
              {search && <button type="button" aria-label="Clear search" className="focus-visible:ring-ring rounded-sm outline-none focus-visible:ring-2" onClick={() => setSearch("")}><X className="text-muted-foreground size-4" aria-hidden="true" /></button>}
            </div>
            <div className="w-full sm:w-56">
              <Select aria-label="Scope" options={scopes.map((item) => ({ value: item.value, label: `${item.label} (${item.count})` }))} value={scopeFilterValue(activeScope)} onValueChange={(next) => setScope(parseScopeFilter(next))} />
            </div>
            <p role="status" data-slot="secrets-count" className="text-muted-foreground text-xs sm:ml-auto">
              {filtered ? `${shown.length} of ${secrets.length}` : secrets.length} {secrets.length === 1 ? "secret" : "secrets"}
            </p>
          </div>
          {!IS_PRIMARY_APP && <p data-slot="secrets-showcase-note" className="text-muted-foreground mb-3 text-xs">Values are revealed and copied in the ADI app itself; this showcase lists, sets and deletes them.</p>}
          {query.isError && <p role="alert" className="text-muted-foreground mb-3 text-xs">Couldn’t refresh just now; showing the last list that loaded.</p>}
          {shown.length ? (
            <Card className="gap-0 p-0 shadow-none">
              <ul aria-label="Secrets" className="divide-y">
                {shown.map((secret) => <SecretRow key={secretId(secret)} secret={secret} projects={projects} actions={actions} />)}
              </ul>
            </Card>
          ) : (
            <div className="rounded-xl border border-dashed py-12 text-center">
              <h4 className="font-medium">No secrets match</h4>
              <Button variant="outline" className="mt-4" onClick={() => { setSearch(""); setScope("all"); }}>Reset filters</Button>
            </div>
          )}
        </>
      )}
    </SettingsSection>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return <div className="min-w-0"><dt className="text-muted-foreground text-xs">{label}</dt><dd className="mt-0.5 text-sm break-words">{children}</dd></div>;
}

function SecretDetail({ source, machine, query, projects, target, creating, actions, authorize }: {
  source?: string; machine: string; query: UseQueryResult<SecretsState>; projects: SettingsProject[];
  /** The secret that is open; `null` while a new one is being written. */
  target: { name: string; project: string | null } | null; creating: boolean; actions: SecretActions;
  authorize: (pending: PendingOAuth, scope: string | null) => void;
}) {
  const navigate = useNavigate();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const secret = !creating && target ? query.data?.secrets.find((item) => secretId(item) === secretId(target)) : undefined;
  const back = <Link to={secretHref()} className="text-muted-foreground hover:text-foreground focus-visible:ring-ring mb-4 inline-flex items-center gap-2 rounded-sm text-sm outline-none focus-visible:ring-2"><ArrowLeft className="size-4" aria-hidden="true" />All secrets</Link>;
  const waiting = { disabled: actions.busy !== null, disabledReason: "Wait for the current action to finish." };

  if (query.isPending) return <>{back}<p role="status" className="text-muted-foreground flex items-center gap-2 py-12 text-sm"><LoaderCircle className="size-4 motion-safe:animate-spin" aria-hidden="true" /> Loading secrets…</p></>;
  if (!query.data) return <>{back}<LoadProblem machine={machine} query={query} /></>;
  if (!creating && target && !secret) {
    return <>{back}<div role="alert" data-slot="secret-missing" className="rounded-xl border border-dashed p-8 text-center"><h3 className="font-medium">No secret named “{target.name}” {target.project ? `in ${projectName(target.project, projects)}` : "in the global scope"}</h3><p className="text-muted-foreground mt-2 text-sm">It is not stored on {machine}. It may have been deleted.</p></div></>;
  }

  if (creating || !secret) {
    return (
      <>
        {back}
        <SettingsSection data-slot="settings-secret" title="New secret" description={description}>
          <Messages actions={actions} />
          <NewSecretForm
            source={source}
            secrets={query.data.secrets}
            initialProject={target?.project ?? null}
            busy={actions.busy?.startsWith("set:") ?? false}
            error={actions.saveError}
            onSave={async (body) => {
              const replaced = query.data.secrets.some((item) => secretId(item) === secretId(body));
              if (!(await actions.set(body))) return false;
              actions.announce(replaced ? `Replaced the value of ${body.name}.` : `Saved ${body.name}.`, true);
              navigate(secretHref(body), { replace: true });
              return true;
            }}
            onAuthorize={authorize}
          />
        </SettingsSection>
      </>
    );
  }

  const value = actions.revealed[secretId(secret)];
  const expiry = expiryText(secret.oauth?.expires_at);
  return (
    <>
      {back}
      <SettingsSection
        data-slot="settings-secret"
        data-secret={secret.name}
        title={<span className="flex flex-wrap items-center gap-x-3 gap-y-2"><span className="font-mono break-all">{secret.name}</span><ScopeBadge secret={secret} projects={projects} /><OAuthBadge secret={secret} /></span>}
        description={secret.description || "No description."}
        actions={(
          <>
            <ValueActions secret={secret} actions={actions} />
            <Button variant="ghost" data-slot="secret-delete" aria-label={`Delete ${secret.name}`} {...waiting} onClick={() => { actions.clear(); setConfirmDelete(true); }}><Trash2 className="size-3.5" aria-hidden="true" />Delete…</Button>
          </>
        )}
      >
        <Messages actions={actions} quiet={confirmDelete} />
        {value !== undefined && <div className="mb-6"><RevealedValue value={value} /><p className="text-muted-foreground mt-1.5 text-xs">Shown for a minute, then put away again.</p></div>}
        <dl data-slot="secret-facts" className="mb-8 grid gap-x-6 gap-y-4 sm:grid-cols-2 lg:grid-cols-3">
          <Fact label="Injected as"><span className="font-mono">${secret.name}</span></Fact>
          <Fact label="Scope">{secret.project ? <>Project {projectName(secret.project, projects)}</> : "Global — every run"}</Fact>
          <Fact label="Updated"><span title={fullDate(secret.updated_at)}>{formatAge(secret.updated_at)}</span></Fact>
          <Fact label="Created"><span title={fullDate(secret.created_at)}>{formatAge(secret.created_at)}</span></Fact>
          {secret.oauth && <Fact label="Signed in with">{providerLabel(secret.oauth.provider)}{expiry ? ` · ${expiry}` : ""}</Fact>}
          {secret.oauth?.scope && <Fact label="Access granted"><span className="font-mono text-xs break-all">{secret.oauth.scope}</span></Fact>}
        </dl>

        {secret.oauth && (
          <div data-slot="secret-oauth-actions" className="mb-8 rounded-lg border p-4">
            <h4 className="text-sm font-medium">{providerLabel(secret.oauth.provider)} sign-in</h4>
            <p className="text-muted-foreground mt-1 text-sm leading-relaxed">
              {secret.oauth.has_refresh ? "The machine holds a refresh token and can get a new access token without you." : "No refresh token is stored, so a new access token needs you to sign in again."}
            </p>
            <div className="mt-3 flex flex-wrap gap-2">
              {secret.oauth.has_refresh && (
                <Button variant="outline" size="sm" data-slot="secret-refresh" {...waiting} onClick={() => void actions.refresh(secret).then((state) => state && actions.announce(`Refreshed ${secret.name}.`))}>
                  {actions.busy === `refresh:${secretId(secret)}` ? <LoaderCircle className="size-3.5 motion-safe:animate-spin" aria-hidden="true" /> : <RotateCw className="size-3.5" aria-hidden="true" />}
                  Refresh token
                </Button>
              )}
              <Button variant="outline" size="sm" data-slot="secret-reauthorize" disabled={!IS_PRIMARY_APP} disabledReason="Sign-in returns to the ADI app itself. Open this page there to authorize."
                onClick={() => { authorize({ ...secretRef(secret), description: secret.description || null, provider: secret.oauth!.provider, ...(source ? { source } : {}) }, secret.oauth!.scope ?? null); actions.announce(`The ${providerLabel(secret.oauth!.provider)} sign-in opened in a new tab. When it finishes, that tab returns here with ${secret.name} stored again.`); }}>
                Re-authorize
              </Button>
            </div>
          </div>
        )}

        <h4 className="mb-4 text-sm font-medium">Replace the value</h4>
        <ReplaceSecretForm
          // The form starts over for another secret, and after this one was saved elsewhere.
          key={`${secretId(secret)}:${secret.updated_at}`}
          secret={secret}
          busy={actions.busy === `set:${secretId(secret)}`}
          error={confirmDelete ? null : actions.saveError}
          onSave={async (body) => {
            if (!(await actions.set(body))) return false;
            actions.announce(`Replaced the value of ${body.name}.`);
            return true;
          }}
        />
      </SettingsSection>
      {confirmDelete && (
        <ConfirmDialog
          title={`Delete ${secret.name}`}
          confirmLabel="Delete secret"
          busy={actions.busy !== null}
          error={actions.error}
          onClose={() => { actions.clear(); setConfirmDelete(false); }}
          onConfirm={async () => {
            if (!(await actions.remove(secret))) return;
            actions.announce(`Deleted ${secret.name}.`, true);
            navigate(secretHref(), { replace: true });
          }}
        >
          <p>Delete the secret “{secret.name}” ({secret.project ? `project ${projectName(secret.project, projects)}` : "global"}) from {machine}? Runs that expect <span className="font-mono">${secret.name}</span> will no longer get it{secret.oauth ? `, and its ${providerLabel(secret.oauth.provider)} sign-in is dropped` : ""}. The value is not kept anywhere: this cannot be undone.</p>
        </ConfirmDialog>
      )}
    </>
  );
}

function SecretsSection({ source }: { source?: string }) {
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const name = params.get("secret");
  const project = params.get("project") || null;
  const creating = name === null && params.get("new") === "1";
  const machine = source ? sourceName(source) : "this machine";
  const query = useQuery({ queryKey: secretsKey(source), queryFn: loadSecrets(source), refetchInterval: 30000, retry: false });
  const projects = useQuery({ queryKey: settingsProjectsKey(source), queryFn: loadSettingsProjects(source), staleTime: 30000, retry: false });
  const actions = useSecretActions(source);
  const place = `${name ?? ""}|${project ?? ""}|${creating}`;
  const first = useRef(true);

  useEffect(() => {
    if (first.current) { first.current = false; return; }
    actions.moved();
  }, [place]);

  // A sign-in returning from the provider: the token is in this address's fragment. It is taken
  // out of the address before anything else, stored against the intent that was parked when the
  // sign-in started, and never kept in this page.
  useEffect(() => {
    const result = parseOAuthReturn(window.location.hash);
    if (result.kind === "none") return;
    const pending = takeOAuth();
    window.history.replaceState(window.history.state, "", `${window.location.pathname}${window.location.search}`);
    navigate(secretHref(), { replace: true });
    if (result.kind === "error") { actions.fail(`The sign-in did not finish: ${result.message}`); return; }
    if (!pending) { actions.fail("A sign-in returned here, but this browser has no record of which secret it was for. Start it again from New secret."); return; }
    void actions.setOAuth(pending.source, oauthSecretBody(pending, result)).then((state) => {
      if (state) actions.announce(`Stored ${pending.name} from ${providerLabel(result.provider ?? pending.provider)}${pending.source && pending.source !== source ? ` on ${sourceName(pending.source)}` : ""}.`);
    });
  }, []);

  function authorize(pending: PendingOAuth, scope: string | null) {
    parkOAuth(pending);
    // Opened in the click itself, so it counts as the user's own; a new tab keeps this page where it is.
    window.open(oauthLoginUrl(pending.provider, `${window.location.origin}${secretHref()}`, scope), "_blank", "noopener");
  }

  const known = projects.data ?? [];
  if (name !== null || creating) return <SecretDetail source={source} machine={machine} query={query} projects={known} target={name !== null ? { name, project } : project ? { name: "", project } : null} creating={creating} actions={actions} authorize={authorize} />;
  return <SecretList machine={machine} query={query} projects={known} actions={actions} />;
}

export function SecretsSettings() {
  const [source] = useSettingsSource();
  // Another machine has other secrets, and nothing revealed on one may linger on the next.
  return <SecretsSection key={source ?? ""} source={source} />;
}
