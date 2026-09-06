//! The Secrets page: encrypted key-values under `~/.adi/mono/secrets/`, injected into runs.
//!
//! A secret is one KEY→value, stored encrypted at rest, in one of two scopes: **global**
//! (available everywhere) or filed under a **project** (overriding a global of the same name for
//! that project's runs). The page lists names and descriptions only — a value is shown solely
//! after an explicit Reveal from the row's menu, and never persists across a reload.
//!
//! A value can be **typed** or **obtained through an OAuth flow**: choosing OAuth sends the user
//! to the [`oauth-router`](https://oauth-router.withadi.dev) worker, which returns the token in
//! the redirect fragment; the page captures it and stores it via `/api/secrets/set-oauth`. An
//! OAuth secret shows a provider tag with its expiry, and can be **refreshed** (server-side,
//! using its stored refresh token) or **re-authorized**.
//!
//! The table and create form are shared with a project's Secrets panel, so their view helpers are
//! `pub(crate)`.

use adi_ui::{EmptyRow, Row as TableRow, Table};
use adi_webapp_api::types::{OAuthInfoDto, SecretDto, SecretsState, SetOAuthSecret, SetSecret};
use leptos::prelude::*;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::Route;
use crate::state::{Flash, SecretsForm, State};
use crate::ui::{
    Key, TableState, TextField, apply_mutation, confirm, flash_view, fmt_date, menu_item,
    row_actions, sort_rows, updated_text,
};

/// The localStorage key holding the secret we're mid-OAuth for, across the provider round-trip.
const PENDING_KEY: &str = "adi.oauth.pending";

/// The columns of the global secrets table.
pub(crate) const COLS: &[&str] = &["Name", "Value", "Description", "Project", "Updated", ""];

/// The same in a project's own Secrets panel, which drops the Project column — every row there is
/// scoped to the open project, so it would say the same thing all the way down.
pub(crate) const PROJECT_COLS: &[&str] = &["Name", "Value", "Description", "Updated", ""];

/// The Secrets page: the live secrets table (global scope), then the create form.
pub(crate) fn secrets_view(state: State, form: SecretsForm) -> AnyView {
    // Returning from an OAuth flow, the token arrives in this page's URL fragment — capture it
    // once on mount (the parked intent is consumed, so a refresh can't re-submit).
    handle_oauth_return(state);

    let State {
        secrets,
        secs_since,
        ..
    } = state;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-muted">
                    {move || secrets.get().map_or_else(|| "\u{2014}".to_string(), |s| {
                        let n = s.secrets.iter().filter(|x| x.project.is_none()).count();
                        if n == 1 { "1 global secret".to_string() } else { format!("{n} global secrets") }
                    })}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(secrets, secs_since)}</span>
            </div>
            <Table state=state.tables.secrets>{move || rows_view(state, state.tables.secrets, form, None)}</Table>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"New secret"</h2>
            </div>
            {secret_create_form(state, form, None)}
            {flash_view(state.flash)}
        </section>
    }
    .into_any()
}

/// The create form, shared by the global page and a project's panel. `project` fixes the secret's
/// scope (the project panel passes `Some(id)`); the global form lets the user type one. A source
/// toggle switches the value between a typed field and an OAuth provider flow.
pub(crate) fn secret_create_form(
    state: State,
    form: SecretsForm,
    project: Option<String>,
) -> AnyView {
    let text_project = project.clone();
    let oauth_project = project.clone();
    view! {
        <div class="adi-form">
            <div class="adi-field">
                <span class="adi-field__label">"Value from"</span>
                <div class="adi-segmented" role="group" aria-label="Where the value comes from">
                    {source_toggle_button(form, "text", "Typed")}
                    {source_toggle_button(form, "oauth", "OAuth")}
                </div>
            </div>
            <TextField id="secret-name" label="Name" mono=true placeholder="API_KEY"
                hint="The env var it is injected as: letters, digits, _." value=form.name />
            <TextField id="secret-desc" label="Description" wide=true field_class="adi-field--grow"
                placeholder="What it's for" value=form.description />
            {(project.is_none()).then(move || view! {
                <TextField id="secret-project" label="Project" mono=true
                    placeholder="global" hint="A project id scopes it to that project's runs."
                    value=form.project />
            })}
            {move || if form.source.get() == "oauth" {
                oauth_authorize_row(state, form, oauth_project.clone())
            } else {
                text_value_row(state, form, text_project.clone())
            }}
        </div>
    }
    .into_any()
}

/// One option of the Typed | OAuth source control.
fn source_toggle_button(form: SecretsForm, value: &'static str, label: &'static str) -> AnyView {
    view! {
        <button type="button" class="adi-segmented__option"
            aria-pressed=move || (form.source.get() == value).to_string()
            on:click=move |_| form.source.set(value.to_string())>
            {label}
        </button>
    }
    .into_any()
}

/// The typed-value branch of the create form: a masked value field + the Set button.
fn text_value_row(state: State, form: SecretsForm, project: Option<String>) -> AnyView {
    view! {
        <div class="adi-field adi-field--grow">
            <label class="adi-field__label" for="secret-value">"Value"</label>
            <input class="adi-input adi-input--wide adi-mono" id="secret-value" type="password"
                autocomplete="off" spellcheck="false" placeholder="Stored encrypted"
                prop:value=move || form.value.get()
                on:input=move |ev| form.value.set(event_target_value(&ev)) />
        </div>
        <button class="adi-btn adi-btn--primary" type="button" prop:disabled=move || form.busy.get()
            on:click=move |_| submit_text(state, form, project.clone())>
            "Set secret"
        </button>
    }
    .into_any()
}

/// The OAuth branch of the create form: a provider picker, the per-provider access checkboxes,
/// and the Authorize button.
fn oauth_authorize_row(state: State, form: SecretsForm, project: Option<String>) -> AnyView {
    view! {
        <div class="adi-field">
            <label class="adi-field__label" for="secret-provider">"Provider"</label>
            <select class="adi-input" id="secret-provider"
                on:change=move |ev| form.provider.set(event_target_value(&ev))>
                <option value="google">"Google"</option>
                <option value="github">"GitHub"</option>
            </select>
        </div>
        {move || access_checkboxes(form)}
        <button class="adi-btn adi-btn--primary" type="button"
            on:click=move |_| start_oauth(state, form, project.clone())>
            {move || format!("Authorize with {}", provider_label(&current_provider(form)))}
        </button>
        <p class="adi-hint">
            "You'll be sent to " <code>"oauth-router.withadi.dev"</code>
            " to sign in, then returned here with the token stored."
        </p>
    }
    .into_any()
}

/// The selectable access scopes for a provider, as `(scope, label)` — rendered as checkboxes.
/// These are *requested*; the provider returns what it actually granted, which is what gets
/// stored on the secret. An empty list means "no picker — use the provider's default scopes".
/// Extend a provider (or add one) by adding rows here.
fn provider_accesses(provider: &str) -> &'static [(&'static str, &'static str)] {
    match provider {
        "google" => &[
            (
                "https://www.googleapis.com/auth/gmail.readonly",
                "Gmail — read (messages, threads, labels)",
            ),
            (
                "https://www.googleapis.com/auth/gmail.send",
                "Gmail — send email",
            ),
            (
                "https://www.googleapis.com/auth/gmail.modify",
                "Gmail — read, send and manage (labels, drafts, archive; no permanent delete)",
            ),
            (
                "https://mail.google.com/",
                "Gmail — full access (including permanent delete)",
            ),
            ("email", "Account email address"),
        ],
        _ => &[],
    }
}

/// The access checkbox group for the currently-selected provider (nothing when it has no
/// catalog — the provider's default scopes are used then).
fn access_checkboxes(form: SecretsForm) -> AnyView {
    let accesses = provider_accesses(&current_provider(form));
    if accesses.is_empty() {
        return ().into_any();
    }
    let boxes = accesses
        .iter()
        .map(|(scope, label)| access_checkbox(form, scope, label))
        .collect::<Vec<_>>();
    view! {
        <fieldset class="adi-field adi-secrets__access">
            <legend class="adi-field__label">"Access"</legend>
            {boxes}
        </fieldset>
    }
    .into_any()
}

/// One access checkbox, toggling membership of `scope` in the form's requested-scope set.
fn access_checkbox(form: SecretsForm, scope: &'static str, label: &'static str) -> AnyView {
    let contains = move || form.scopes.get().iter().any(|s| s == scope);
    view! {
        <label class="adi-field--check adi-secrets__scope">
            <input type="checkbox" class="adi-check" prop:checked=contains
                on:change=move |ev| {
                    let on = event_target_checked(&ev);
                    form.scopes.update(|list| {
                        if on {
                            if !list.iter().any(|s| s == scope) {
                                list.push(scope.to_string());
                            }
                        } else {
                            list.retain(|s| s != scope);
                        }
                    });
                } />
            <span>{label}</span>
        </label>
    }
    .into_any()
}

/// Post a typed secret from the create form (the original Text-source behavior).
fn submit_text(state: State, form: SecretsForm, scoped: Option<String>) {
    let name = form.name.get().trim().to_string();
    if name.is_empty() {
        state
            .flash
            .set(Some(Flash::err("A secret name is required.".to_string())));
        return;
    }
    let value = form.value.get();
    let description = form.description.get().trim().to_string();
    let description = (!description.is_empty()).then_some(description);
    let project = resolve_scope(form, scoped.as_ref());
    let body = SetSecret {
        project,
        name: name.clone(),
        value,
        description,
    };
    reset_form(form);
    form.busy.set(true);
    apply_mutation(
        state,
        Some(form.busy),
        format!("Set secret \u{201c}{name}\u{201d}."),
        |s: State, sec: SecretsState| s.secrets.set(Some(sec)),
        fetch::set_secret(body),
    );
}

/// Begin the OAuth flow for the secret named in the create form: park the intent, then leave for
/// the provider. On return, [`handle_oauth_return`] stores the token.
fn start_oauth(state: State, form: SecretsForm, scoped: Option<String>) {
    let name = form.name.get().trim().to_string();
    if name.is_empty() {
        state
            .flash
            .set(Some(Flash::err("A secret name is required.".to_string())));
        return;
    }
    let provider = current_provider(form);
    // For a provider with an access catalog (Google/Gmail), request exactly the ticked scopes;
    // require at least one. A provider without a catalog uses its default scopes.
    let requested_scope = if provider_accesses(&provider).is_empty() {
        None
    } else {
        let selected = form.scopes.get();
        if selected.is_empty() {
            state
                .flash
                .set(Some(Flash::err("Select at least one access.".to_string())));
            return;
        }
        Some(selected.join(" "))
    };
    let description = form.description.get().trim().to_string();
    let description = (!description.is_empty()).then_some(description);
    oauth_initiate(
        &PendingOAuth {
            name,
            project: resolve_scope(form, scoped.as_ref()),
            description,
            provider,
        },
        requested_scope.as_deref(),
    );
}

/// The scope a form submit targets: a panel-fixed project wins, else the typed project field
/// (blank ⇒ global).
fn resolve_scope(form: SecretsForm, scoped: Option<&String>) -> Option<String> {
    if let Some(id) = scoped {
        return Some(id.clone());
    }
    let p = form.project.get().trim().to_string();
    (!p.is_empty()).then_some(p)
}

/// The provider currently selected, defaulting to `google`.
fn current_provider(form: SecretsForm) -> String {
    let p = form.provider.get();
    if p.is_empty() {
        "google".to_string()
    } else {
        p
    }
}

/// Render a secrets table body: the loading/empty placeholder, or one row per matching secret, in
/// the order and arrangement `table`'s header controls select. `project` (when `Some`) filters to
/// that project, as a project's own panel does; `None` shows global secrets only.
///
/// Reads the reveal cache up front, so a Reveal rebuilds the rows and the row's menu says Hide.
pub(crate) fn rows_view(
    state: State,
    table: TableState,
    form: SecretsForm,
    project: Option<String>,
) -> AnyView {
    let Some(loaded) = state.secrets.get() else {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    };
    let revealed = form.revealed.get();
    let want = project;
    let mut rows: Vec<SecretDto> = loaded
        .secrets
        .into_iter()
        .filter(|s| s.project.as_deref() == want.as_deref())
        .collect();
    if rows.is_empty() {
        return view! { <EmptyRow state=table>"No secrets yet — set one below."</EmptyRow> }
            .into_any();
    }
    sort_rows(
        &mut rows,
        table.sort.get(),
        |s, col| match col {
            // Never by the value — it isn't held here, and sorting by a masked cell would order
            // by nothing at all. Grouping the OAuth-sourced secrets is the useful order instead.
            "Value" => Key::maybe(s.oauth.as_ref().map(|o| o.provider.as_str())),
            "Description" => Key::maybe(s.description.as_deref()),
            "Project" => Key::maybe(s.project.as_deref()),
            "Updated" => Key::num(s.updated_at),
            _ => Key::text(&s.name),
        },
        |s| Key::text(&s.name),
    );
    rows.into_iter()
        .map(|s| {
            let shown = revealed
                .get(&reveal_key(s.project.as_deref(), &s.name))
                .cloned();
            let action = secret_actions(state, form, &s, shown.is_some());
            view! { <TableRow state=table cell=move |col| cell(col, &s, shown.clone()) actions=action/> }
                .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One secret's cell under `col`: the name (with an OAuth tag when applicable), the value as a
/// `● set` pill or — after a Reveal — the plaintext, its description, the project it is scoped
/// to, and when it last changed. Matching the header text — the same key the sort uses — is what lets the user hide
/// and reorder columns without the row builder knowing about it, and is also how the project
/// panel's narrower column set works from the same code.
fn cell(col: &str, s: &SecretDto, shown: Option<String>) -> AnyView {
    match col {
        "Value" => match shown {
            Some(v) => view! { <span class="adi-mono">{v}</span> }.into_any(),
            None => view! { <span class="adi-tstatus" data-status="ready">"set"</span> }.into_any(),
        },
        "Description" => {
            let desc = s.description.clone().unwrap_or_default();
            if desc.is_empty() {
                view! { <span class="adi-muted">"—"</span> }.into_any()
            } else {
                view! { <span class="adi-secrets__desc">{desc}</span> }.into_any()
            }
        }
        // The scope is a category, so it is a tag; a global secret has none.
        "Project" => match s.project.clone() {
            Some(project) => view! { <span class="adi-chip">{project}</span> }.into_any(),
            None => view! { <span class="adi-muted">"—"</span> }.into_any(),
        },
        "Updated" => {
            view! { <span class="adi-muted adi-tabnums">{fmt_date(s.updated_at)}</span> }.into_any()
        }
        // "Name", and anything the layout offers that this match doesn't name.
        _ => {
            let badge = s.oauth.as_ref().map(oauth_badge);
            view! {
                <span class="adi-secrets__name">
                    <span class="adi-mono adi-secrets__env">{s.name.clone()}</span>
                    {badge}
                </span>
            }
            .into_any()
        }
    }
}

/// The trailing actions for a secret row — shared by the global table and a project's panel: one
/// ⋯ menu holding Reveal (or Hide, while the value is showing), the OAuth Refresh and Re-authorize
/// when applicable, and the destructive Remove.
fn secret_actions(state: State, form: SecretsForm, s: &SecretDto, revealed: bool) -> AnyView {
    let key = reveal_key(s.project.as_deref(), &s.name);
    let toggle_key = key.clone();
    let reveal_project = s.project.clone();
    let reveal_name = s.name.clone();
    let reveal = menu_item(
        state,
        if revealed {
            "Hide value"
        } else {
            "Reveal value"
        },
        false,
        move || {
            if revealed {
                form.revealed.update(|m| {
                    m.remove(&toggle_key);
                });
            } else {
                reveal_now(state, form, reveal_project.clone(), reveal_name.clone());
            }
        },
    );
    let mut items = vec![reveal];
    items.extend(secret_menu_items(state, s));
    row_actions(state, format!("secret:{key}"), (), items)
}

/// The menu items after Reveal: the OAuth actions (Refresh when a refresh token is held,
/// Re-authorize) and the destructive Remove.
fn secret_menu_items(state: State, s: &SecretDto) -> Vec<AnyView> {
    let mut items = Vec::new();
    if let Some(info) = s.oauth.as_ref() {
        if info.has_refresh {
            let (refresh_name, refresh_project) = (s.name.clone(), s.project.clone());
            items.push(menu_item(state, "Refresh token", false, move || {
                apply_secrets(
                    state,
                    format!("Refreshed \u{201c}{refresh_name}\u{201d}."),
                    fetch::refresh_secret(refresh_project.clone(), refresh_name.clone()),
                );
            }));
        }
        // Re-authorize asking for the same access the secret already holds.
        let reauth = PendingOAuth {
            name: s.name.clone(),
            project: s.project.clone(),
            description: s.description.clone(),
            provider: info.provider.clone(),
        };
        let reauth_scope = info.scope.clone();
        items.push(menu_item(state, "Re-authorize", false, move || {
            oauth_initiate(&reauth, reauth_scope.as_deref());
        }));
    }
    let (del_project, del_name, del_display) = (s.project.clone(), s.name.clone(), s.name.clone());
    items.push(menu_item(state, "Remove", true, move || {
        if !confirm(&format!(
            "Delete secret {del_display}? This is irreversible."
        )) {
            return;
        }
        apply_secrets(
            state,
            "Deleted secret.".to_string(),
            fetch::remove_secret(del_project.clone(), del_name.clone()),
        );
    }));
    items
}

/// The provenance tag on an OAuth secret's row: the provider and a coarse expiry.
fn oauth_badge(info: &OAuthInfoDto) -> AnyView {
    let label = match expiry_text(info.expires_at) {
        Some(exp) => format!("{} · {exp}", provider_label(&info.provider)),
        None => provider_label(&info.provider),
    };
    view! { <span class="adi-chip" title="Obtained through an OAuth sign-in">{label}</span> }
        .into_any()
}

/// A coarse "expires in Nm/Nh" / "expired" label from an absolute expiry, or `None` when the
/// provider gave no expiry.
fn expiry_text(expires_at: Option<u64>) -> Option<String> {
    let exp = expires_at?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let now = (js_sys::Date::now() / 1000.0) as u64;
    if exp <= now {
        return Some("expired".to_string());
    }
    let mins = (exp - now) / 60;
    Some(if mins < 60 {
        format!("expires in {mins}m")
    } else {
        format!("expires in {}h", mins / 60)
    })
}

/// A display name for a provider id.
pub(crate) fn provider_label(provider: &str) -> String {
    match provider {
        "google" => "Google".to_string(),
        "github" => "GitHub".to_string(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        }
    }
}

// ── The OAuth capture flow ──────────────────────────────────────────────────────────────────

/// A secret we're mid-OAuth for, parked in sessionStorage across the provider round-trip.
#[derive(serde::Serialize, serde::Deserialize)]
struct PendingOAuth {
    name: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    description: Option<String>,
    provider: String,
}

/// Park `pending`, then open the router's login for its provider in a **new tab**, requesting
/// `requested_scope` (the ticked access scopes) when given. The new tab runs the whole round-trip
/// and returns to `<origin>/secrets` with the token in its URL fragment, where the Secrets page
/// captures it; the tab you started from stays put.
///
/// Two reasons this opens a new tab rather than navigating in place:
/// - the flow no longer hijacks the chat/page you launched it from; and
/// - the new tab visits `oauth-router.withadi.dev` as its own top-level site instead of a transient
///   redirect hop out of `app.adi`, which keeps browsers (Safari ITP "bounce tracking" in
///   particular) from purging the router's CSRF nonce cookie mid-flow — the cause of a
///   `state / cookie mismatch` on return.
///
/// Because the new tab is a fresh browsing context, the parked intent must be readable there — so
/// it lives in **localStorage** (shared across the origin's tabs), not tab-scoped sessionStorage.
fn oauth_initiate(pending: &PendingOAuth, requested_scope: Option<&str>) {
    if let (Some(store), Ok(json)) = (oauth_store(), serde_json::to_string(pending)) {
        let _ = store.set_item(PENDING_KEY, &json);
    }
    let Some(win) = web_sys::window() else {
        return;
    };
    let origin = win.location().origin().unwrap_or_default();
    // Return to the Secrets page (now under `/extended`) so the App shell mounts and captures
    // the token fragment — landing on the bare root would silently drop it.
    let redirect: String =
        js_sys::encode_uri_component(&format!("{origin}{}", Route::Secrets.path())).into();
    let mut url = format!(
        "{}/login/{}?redirect={redirect}",
        crate::links::OAUTH_ROUTER,
        pending.provider
    );
    if let Some(scope) = requested_scope.filter(|s| !s.trim().is_empty()) {
        let enc: String = js_sys::encode_uri_component(scope).into();
        url.push_str("&scope=");
        url.push_str(&enc);
    }
    // A synchronous open in the click/submit handler counts as a user gesture, so it isn't blocked.
    let _ = win.open_with_url_and_target(&url, "_blank");
}

/// If this page load is a return from the router (token or error in the fragment), store the
/// token against the parked intent and scrub the URL. A no-op otherwise. The intent is consumed,
/// so a manual refresh of the returned URL can't re-submit.
fn handle_oauth_return(state: State) {
    let Some(win) = web_sys::window() else {
        return;
    };
    let hash = win.location().hash().unwrap_or_default();
    let frag = hash.trim_start_matches('#');
    if frag.is_empty() {
        return;
    }
    let Ok(params) = web_sys::UrlSearchParams::new_with_str(frag) else {
        return;
    };
    if params.get("access_token").is_none() && params.get("error").is_none() {
        return;
    }

    // A genuine OAuth return: consume the parked intent and scrub the fragment either way.
    let pending = take_pending();
    clear_fragment(&win);
    let Some(pending) = pending else {
        return;
    };

    if let Some(err) = params.get("error") {
        let desc = params.get("error_description").unwrap_or(err);
        state
            .flash
            .set(Some(Flash::err(format!("OAuth failed: {desc}"))));
        return;
    }

    let access_token = params.get("access_token").unwrap_or_default();
    if access_token.is_empty() {
        return;
    }
    let body = SetOAuthSecret {
        project: pending.project,
        name: pending.name.clone(),
        description: pending.description,
        provider: params.get("provider").unwrap_or(pending.provider),
        access_token,
        refresh_token: params.get("refresh_token").filter(|s| !s.is_empty()),
        expires_in: params.get("expires_in").and_then(|s| s.parse::<u64>().ok()),
        scope: params.get("scope").filter(|s| !s.is_empty()),
    };
    let name = pending.name.clone();
    spawn_local(async move {
        match fetch::set_oauth_secret(body).await {
            Ok(sec) => {
                state.secrets.set(Some(sec));
                state.flash.set(Some(Flash::ok(format!(
                    "Stored OAuth secret \u{201c}{name}\u{201d}."
                ))));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Read and remove the parked OAuth intent (one-shot).
fn take_pending() -> Option<PendingOAuth> {
    let store = oauth_store()?;
    let json = store.get_item(PENDING_KEY).ok().flatten()?;
    let _ = store.remove_item(PENDING_KEY);
    serde_json::from_str(&json).ok()
}

/// Where the mid-OAuth intent is parked: **localStorage**, so the new tab that runs the flow (a
/// separate browsing context) can read it — sessionStorage is per-tab and wouldn't carry across.
/// Consumed one-shot by [`take_pending`] on return.
fn oauth_store() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Strip the token fragment from the URL so a refresh can't replay it and nothing lingers.
fn clear_fragment(win: &web_sys::Window) {
    if let Ok(history) = win.history() {
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(Route::Secrets.path()));
    }
}

/// Fetch and cache one secret's decrypted value, so its row shows it until Hide or a reload.
fn reveal_now(state: State, form: SecretsForm, project: Option<String>, name: String) {
    let key = reveal_key(project.as_deref(), &name);
    spawn_local(async move {
        match fetch::reveal_secret(project, name).await {
            Ok(r) => form.revealed.update(|m| {
                m.insert(key, r.value);
            }),
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Fold a secrets mutation's fresh [`SecretsState`] into the page and flash success or the error.
fn apply_secrets<F>(state: State, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<SecretsState, String>> + 'static,
{
    apply_mutation(state, None, ok_msg, |s, sec| s.secrets.set(Some(sec)), fut);
}

/// Clear the create form's inputs after a submit (keeping the project + source + provider).
fn reset_form(form: SecretsForm) {
    form.name.set(String::new());
    form.value.set(String::new());
    form.description.set(String::new());
}

/// The reveal-cache key for a secret: its scope and name joined by a byte that can't appear in
/// either, so a global and a project secret of the same name never collide in the cache.
fn reveal_key(project: Option<&str>, name: &str) -> String {
    format!("{}\u{1}{name}", project.unwrap_or_default())
}
