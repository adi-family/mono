import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowUpRight, Eye, EyeOff, LoaderCircle } from "lucide-react";
import { Link } from "react-router-dom";

import { Button, Field, Input, Select, Textarea, cn, type FieldControlProps } from "@design-system/ui";

import { IS_PRIMARY_APP } from "../../lib/app-api";
import {
  oauthProviders, projectName, providerAccesses, providerLabel, secretHref, secretId, secretNameProblem,
  type PendingOAuth, type Secret, type SetSecret,
} from "../../lib/secrets";
import { loadSettingsProjects, settingsProjectsKey } from "../../lib/settings-api";

/**
 * A value being typed in: masked like a password, but a textarea, so a multi-line value — a
 * private key, a JSON credential — arrives whole instead of having its line breaks stripped.
 */
export function SecretValueInput({ control, value, onChange }: { control: FieldControlProps; value: string; onChange: (value: string) => void }) {
  const [shown, setShown] = useState(false);
  return (
    <div className="flex items-start gap-1.5">
      <Textarea
        {...control}
        mono
        name="value"
        rows={3}
        autoComplete="off"
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
        data-1p-ignore=""
        data-lpignore="true"
        data-masked={!shown}
        placeholder="Stored encrypted"
        className={cn("min-h-16", !shown && "[-webkit-text-security:disc]")}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
      <Button type="button" variant="ghost" size="icon" aria-label={shown ? "Hide the value" : "Show the value"} aria-pressed={shown} onClick={() => setShown((current) => !current)}>
        {shown ? <EyeOff className="size-4" aria-hidden="true" /> : <Eye className="size-4" aria-hidden="true" />}
      </Button>
    </div>
  );
}

const oauthOnlyHere = "Sign-in returns to the ADI app itself. Open this page there to authorize.";

/** A new secret: typed in, or obtained by signing in to a provider. */
export function NewSecretForm({ source, secrets, initialProject, busy, error, onSave, onAuthorize }: {
  source?: string;
  /** The secrets already on the machine, to say when a save would replace one. */
  secrets: Secret[];
  initialProject: string | null;
  busy: boolean;
  error: string | null;
  onSave: (body: SetSecret) => Promise<boolean>;
  /** Park the intent and open the provider's sign-in in a new tab. */
  onAuthorize: (pending: PendingOAuth, scope: string | null) => void;
}) {
  const [kind, setKind] = useState<"typed" | "oauth">("typed");
  const [name, setName] = useState("");
  const [project, setProject] = useState(initialProject ?? "");
  const [description, setDescription] = useState("");
  const [value, setValue] = useState("");
  const [provider, setProvider] = useState<string>(oauthProviders[0].value);
  const [scopes, setScopes] = useState<string[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  const [opened, setOpened] = useState<string | null>(null);
  const projects = useQuery({ queryKey: settingsProjectsKey(source), queryFn: loadSettingsProjects(source), staleTime: 30000, retry: false });
  const known = projects.data ?? [];
  const scopeOptions = [
    { value: "", label: "Global — available to every run" },
    ...known.map((item) => ({ value: item.id, label: item.name })),
    ...(project && !known.some((item) => item.id === project) ? [{ value: project, label: project }] : []),
  ];
  const trimmed = name.trim();
  const existing = secrets.find((secret) => secretId(secret) === secretId({ name: trimmed, project }));
  const accesses = providerAccesses(provider);
  const edit = <T,>(set: (next: T) => void) => (next: T) => { setProblem(null); setOpened(null); set(next); };

  function check(): boolean {
    const found = secretNameProblem(name)
      ?? (kind === "typed" && !value ? "Enter the value to store."
        : kind === "oauth" && accesses.length > 0 && scopes.length === 0 ? "Select at least one access."
          : null);
    setProblem(found);
    return !found;
  }

  return (
    <form
      data-slot="secret-form"
      aria-busy={busy}
      noValidate
      className="space-y-6"
      onSubmit={async (event) => {
        event.preventDefault();
        if (!check()) return;
        if (kind === "typed") { await onSave({ name: trimmed, project: project || null, value, description: description.trim() || null }); return; }
        onAuthorize({ name: trimmed, project: project || null, description: description.trim() || null, provider, ...(source ? { source } : {}) }, accesses.length ? scopes.join(" ") : null);
        setOpened(provider);
      }}
    >
      <div role="group" aria-label="Where the value comes from" className="flex flex-wrap gap-1">
        {([["typed", "Typed value"], ["oauth", "OAuth sign-in"]] as const).map(([id, label]) => (
          <Button key={id} type="button" variant="ghost" size="sm" data-kind={id} aria-pressed={kind === id} className={cn(kind === id && "bg-muted text-foreground")} onClick={() => edit(setKind)(id)}>{label}</Button>
        ))}
      </div>

      <div className="grid gap-x-5 gap-y-5 sm:grid-cols-2">
        <Field label="Name" hint="The environment variable it is injected as: letters, digits and underscores.">
          {(control) => <Input {...control} mono name="name" required autoComplete="off" spellCheck={false} placeholder="API_KEY" value={name} onChange={(event) => edit(setName)(event.target.value)} />}
        </Field>
        <Field label="Scope" hint={projects.isError ? "Couldn’t load the projects; Global is still available." : "A project’s secret reaches only that project’s runs, and overrides a global one of the same name."}>
          {(control) => <Select {...control} name="project" options={scopeOptions} value={project} onValueChange={edit(setProject)} />}
        </Field>
        <Field label="Description" hint="What it is for. Shown in the list; never the value." className="sm:col-span-2">
          {(control) => <Input {...control} name="description" autoComplete="off" placeholder="What it’s for" value={description} onChange={(event) => edit(setDescription)(event.target.value)} />}
        </Field>
        {kind === "typed" ? (
          <Field label="Value" hint="Encrypted at rest on the machine. It is not shown again unless you reveal it." className="sm:col-span-2">
            {(control) => <SecretValueInput control={control} value={value} onChange={edit(setValue)} />}
          </Field>
        ) : (
          <>
            <Field label="Provider">
              {(control) => <Select {...control} name="provider" options={oauthProviders} value={provider} onValueChange={(next) => { edit(setProvider)(next); setScopes([]); }} />}
            </Field>
            {accesses.length > 0 && (
              <fieldset className="min-w-0 space-y-2 sm:col-span-2">
                <legend className="text-sm font-medium">Access</legend>
                <p className="text-muted-foreground text-xs">What to ask {providerLabel(provider)} for. The secret records what was actually granted.</p>
                <div data-slot="secret-access" className="space-y-2">
                  {accesses.map((access) => (
                    <label key={access.scope} className="flex items-start gap-2 text-sm">
                      <input type="checkbox" className="accent-primary mt-0.5 size-4 shrink-0" value={access.scope} checked={scopes.includes(access.scope)} onChange={(event) => edit(setScopes)(event.target.checked ? [...scopes, access.scope] : scopes.filter((scope) => scope !== access.scope))} />
                      {access.label}
                    </label>
                  ))}
                </div>
              </fieldset>
            )}
          </>
        )}
      </div>

      {existing && (
        <p role="status" data-slot="secret-exists" className="bg-muted rounded-lg border p-3 text-sm">
          <span className="font-mono">{trimmed}</span> already exists in {existing.project ? projectName(existing.project, known) : "the global scope"}. Saving replaces its value{existing.oauth && kind === "typed" ? ` and drops its ${providerLabel(existing.oauth.provider)} sign-in` : ""}.{" "}
          <Link to={secretHref(existing)} className="underline underline-offset-4">Open it</Link>
        </p>
      )}
      {opened && (
        <p role="status" data-slot="secret-oauth-opened" className="bg-muted rounded-lg border p-3 text-sm">
          The {providerLabel(opened)} sign-in opened in a new tab. When it finishes, that tab returns here with <span className="font-mono">{trimmed}</span> stored, and this list picks it up.
        </p>
      )}

      <div className="flex flex-wrap items-center gap-3 border-t pt-5">
        {kind === "typed" ? (
          <Button type="submit" data-slot="secret-save" disabled={busy} disabledReason="The secret is being saved.">
            {busy && <LoaderCircle className="size-4 motion-safe:animate-spin" aria-hidden="true" />}
            {busy ? "Saving…" : existing ? "Replace existing secret" : "Save secret"}
          </Button>
        ) : (
          <Button type="submit" data-slot="secret-authorize" disabled={!IS_PRIMARY_APP} disabledReason={oauthOnlyHere}>
            Authorize with {providerLabel(provider)}
            <ArrowUpRight className="size-3.5" aria-hidden="true" />
          </Button>
        )}
        <Button asChild variant="ghost"><Link to={secretHref()}>Cancel</Link></Button>
        {(problem ?? error) && <p role="alert" data-slot="secret-form-error" className="text-destructive min-w-0 basis-full text-sm sm:basis-auto">{problem ?? error}</p>}
      </div>
      {kind === "oauth" && (
        <p className="text-muted-foreground text-xs leading-relaxed">
          You sign in at <span className="font-mono">oauth-router.withadi.dev</span> in a new tab — a provider will not redirect to a private <span className="font-mono">.adi</span> name — and the token comes back to this page, which stores it on the machine.
        </p>
      )}
    </form>
  );
}

/** A new value for an existing secret. The name and the scope are what the secret is. */
export function ReplaceSecretForm({ secret, busy, error, onSave }: { secret: Secret; busy: boolean; error: string | null; onSave: (body: SetSecret) => Promise<boolean> }) {
  const [value, setValue] = useState("");
  const [description, setDescription] = useState(secret.description ?? "");
  const [problem, setProblem] = useState<string | null>(null);

  return (
    <form
      data-slot="secret-replace-form"
      aria-busy={busy}
      noValidate
      className="space-y-5"
      onSubmit={async (event) => {
        event.preventDefault();
        if (!value) { setProblem("Enter the new value."); return; }
        // The description is sent along every time: a save without one would clear it.
        if (await onSave({ name: secret.name, project: secret.project || null, value, description: description.trim() || null })) setValue("");
      }}
    >
      <Field label="New value" hint={secret.oauth
        ? `Typing a value in makes this a typed secret: its ${providerLabel(secret.oauth.provider)} sign-in and refresh token are dropped.`
        : "Replaces the stored value. The old one is not kept."}>
        {(control) => <SecretValueInput control={control} value={value} onChange={(next) => { setProblem(null); setValue(next); }} />}
      </Field>
      <Field label="Description" hint="Saved with the new value.">
        {(control) => <Input {...control} name="description" autoComplete="off" placeholder="What it’s for" value={description} onChange={(event) => setDescription(event.target.value)} />}
      </Field>
      <div className="flex flex-wrap items-center gap-3">
        <Button type="submit" variant="outline" data-slot="secret-replace" disabled={busy} disabledReason="The secret is being saved.">
          {busy && <LoaderCircle className="size-4 motion-safe:animate-spin" aria-hidden="true" />}
          {busy ? "Saving…" : "Replace value"}
        </Button>
        {(problem ?? error) && <p role="alert" data-slot="secret-form-error" className="text-destructive min-w-0 text-sm">{problem ?? error}</p>}
      </div>
    </form>
  );
}
