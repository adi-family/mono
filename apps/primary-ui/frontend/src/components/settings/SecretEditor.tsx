import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowUpRight, Eye, EyeOff, LoaderCircle } from "lucide-react";
import { Controller, useForm, type ResolverResult } from "react-hook-form";
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

type NewSecretValues = {
  kind: "typed" | "oauth";
  name: string;
  project: string;
  description: string;
  value: string;
  provider: string;
  scopes: string[];
};

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
  const {
    control, register, handleSubmit, watch, setValue, clearErrors,
    formState: { errors },
  } = useForm<NewSecretValues>({
    defaultValues: {
      kind: "typed",
      name: "",
      project: initialProject ?? "",
      description: "",
      value: "",
      provider: oauthProviders[0].value,
      scopes: [],
    },
    mode: "onSubmit",
    reValidateMode: "onSubmit",
    resolver: (values): ResolverResult<NewSecretValues> => {
      const currentAccesses = providerAccesses(values.provider);
      const found = secretNameProblem(values.name)
        ?? (values.kind === "typed" && !values.value ? "Enter the value to store."
          : values.kind === "oauth" && currentAccesses.length > 0 && values.scopes.length === 0 ? "Select at least one access."
            : null);
      if (found) return { values: {}, errors: { root: { validate: { type: "validate", message: found } } } };
      return { values, errors: {} };
    },
  });
  const [opened, setOpened] = useState<string | null>(null);
  const kind = watch("kind");
  const name = watch("name");
  const project = watch("project");
  const provider = watch("provider");
  const problem = errors.root?.validate?.message ?? null;

  useEffect(() => {
    const subscription = watch(() => {
      clearErrors("root");
      setOpened(null);
    });
    return () => subscription.unsubscribe();
  }, [watch, clearErrors]);

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

  return (
    <form
      data-slot="secret-form"
      aria-busy={busy}
      noValidate
      className="space-y-6"
      onSubmit={handleSubmit(async (values) => {
        const trimmedName = values.name.trim();
        if (values.kind === "typed") {
          await onSave({ name: trimmedName, project: values.project || null, value: values.value, description: values.description.trim() || null });
          return;
        }
        const currentAccesses = providerAccesses(values.provider);
        onAuthorize(
          { name: trimmedName, project: values.project || null, description: values.description.trim() || null, provider: values.provider, ...(source ? { source } : {}) },
          currentAccesses.length ? values.scopes.join(" ") : null,
        );
        setOpened(values.provider);
      })}
    >
      <div role="group" aria-label="Where the value comes from" className="flex flex-wrap gap-1">
        {([["typed", "Typed value"], ["oauth", "OAuth sign-in"]] as const).map(([id, label]) => (
          <Button key={id} type="button" variant="ghost" size="sm" data-kind={id} aria-pressed={kind === id} className={cn(kind === id && "bg-muted text-foreground")} onClick={() => setValue("kind", id)}>{label}</Button>
        ))}
      </div>

      <div className="grid gap-x-5 gap-y-5 sm:grid-cols-2">
        <Field label="Name" hint="The environment variable it is injected as: letters, digits and underscores.">
          {(controlProps) => <Input {...controlProps} mono required autoComplete="off" spellCheck={false} placeholder="API_KEY" {...register("name")} />}
        </Field>
        <Field label="Scope" hint={projects.isError ? "Couldn’t load the projects; Global is still available." : "A project’s secret reaches only that project’s runs, and overrides a global one of the same name."}>
          {(controlProps) => (
            <Controller
              control={control}
              name="project"
              render={({ field }) => <Select {...controlProps} name={field.name} options={scopeOptions} value={field.value} onValueChange={field.onChange} onBlur={field.onBlur} />}
            />
          )}
        </Field>
        <Field label="Description" hint="What it is for. Shown in the list; never the value." className="sm:col-span-2">
          {(controlProps) => <Input {...controlProps} autoComplete="off" placeholder="What it’s for" {...register("description")} />}
        </Field>
        {kind === "typed" ? (
          <Field label="Value" hint="Encrypted at rest on the machine. It is not shown again unless you reveal it." className="sm:col-span-2">
            {(controlProps) => (
              <Controller
                control={control}
                name="value"
                render={({ field }) => <SecretValueInput control={controlProps} value={field.value} onChange={field.onChange} />}
              />
            )}
          </Field>
        ) : (
          <>
            <Field label="Provider">
              {(controlProps) => (
                <Controller
                  control={control}
                  name="provider"
                  render={({ field }) => (
                    <Select
                      {...controlProps}
                      name={field.name}
                      options={oauthProviders}
                      value={field.value}
                      onValueChange={(next) => { field.onChange(next); setValue("scopes", []); }}
                      onBlur={field.onBlur}
                    />
                  )}
                />
              )}
            </Field>
            {accesses.length > 0 && (
              <fieldset className="min-w-0 space-y-2 sm:col-span-2">
                <legend className="text-sm font-medium">Access</legend>
                <p className="text-muted-foreground text-xs">What to ask {providerLabel(provider)} for. The secret records what was actually granted.</p>
                <Controller
                  control={control}
                  name="scopes"
                  render={({ field }) => (
                    <div data-slot="secret-access" className="space-y-2">
                      {accesses.map((access) => (
                        <label key={access.scope} className="flex items-start gap-2 text-sm">
                          <input
                            type="checkbox"
                            className="accent-primary mt-0.5 size-4 shrink-0"
                            value={access.scope}
                            checked={field.value.includes(access.scope)}
                            onChange={(event) => field.onChange(event.target.checked ? [...field.value, access.scope] : field.value.filter((scope) => scope !== access.scope))}
                          />
                          {access.label}
                        </label>
                      ))}
                    </div>
                  )}
                />
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

type ReplaceSecretValues = {
  value: string;
  description: string;
};

/** A new value for an existing secret. The name and the scope are what the secret is. */
export function ReplaceSecretForm({ secret, busy, error, onSave }: { secret: Secret; busy: boolean; error: string | null; onSave: (body: SetSecret) => Promise<boolean> }) {
  const {
    control, register, handleSubmit, setValue, clearErrors,
    formState: { errors },
  } = useForm<ReplaceSecretValues>({
    defaultValues: { value: "", description: secret.description ?? "" },
    mode: "onSubmit",
    reValidateMode: "onSubmit",
    resolver: (values): ResolverResult<ReplaceSecretValues> => {
      if (!values.value) return { values: {}, errors: { root: { validate: { type: "validate", message: "Enter the new value." } } } };
      return { values, errors: {} };
    },
  });
  const problem = errors.root?.validate?.message ?? null;

  return (
    <form
      data-slot="secret-replace-form"
      aria-busy={busy}
      noValidate
      className="space-y-5"
      onSubmit={handleSubmit(async (values) => {
        // The description is sent along every time: a save without one would clear it.
        if (await onSave({ name: secret.name, project: secret.project || null, value: values.value, description: values.description.trim() || null })) {
          setValue("value", "");
        }
      })}
    >
      <Field label="New value" hint={secret.oauth
        ? `Typing a value in makes this a typed secret: its ${providerLabel(secret.oauth.provider)} sign-in and refresh token are dropped.`
        : "Replaces the stored value. The old one is not kept."}>
        {(controlProps) => (
          <Controller
            control={control}
            name="value"
            render={({ field }) => (
              <SecretValueInput
                control={controlProps}
                value={field.value}
                onChange={(next) => { clearErrors("root"); field.onChange(next); }}
              />
            )}
          />
        )}
      </Field>
      <Field label="Description" hint="Saved with the new value.">
        {(controlProps) => <Input {...controlProps} autoComplete="off" placeholder="What it’s for" {...register("description")} />}
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
