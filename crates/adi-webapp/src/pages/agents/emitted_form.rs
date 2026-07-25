//! Rendering **agent-emitted inline forms**.
//!
//! An agent can drop a fenced ```` ```adi-form <json> ``` ```` block into its chat reply; this module
//! turns that block into a live form the human fills in and submits, rendered right where it appears
//! in the transcript. The agent side stays plain text (no tool loop needed), so it works on today's
//! harness backends — the guide teaches the agent the [`AgentForm`] schema.
//!
//! Both current actions target the secrets store. A **text** secret is set in place (the form shows
//! Saving… → ✓). An **OAuth** secret authorizes in a *separate tab* (so the router's CSRF cookie
//! survives — see [`crate::pages::secrets`]); the form shows "opened in a new tab", then flips to ✓
//! when that tab reports the secret was stored via a cross-tab `storage` event. Anything that isn't
//! a valid form block degrades to a normal Markdown code block, so a malformed spec is never lost.

use std::collections::BTreeMap;

use adi_webapp_api::types::{AgentForm, AgentFormAction, AgentFormInput, AgentFormInputKind, SetSecret};
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::pages::secrets::{begin_oauth_secret, clear_oauth_done, provider_label, read_oauth_done};
use crate::state::{Flash, State};
use crate::ui::field_hint;

/// The fence info-string that marks an agent-emitted form block.
const FORM_FENCE: &str = "adi-form";

/// A form's progress after the human acts on it: idle (show the submit button), a pending line, or a
/// terminal ✓. Held in a signal so the footer re-renders as the action resolves.
#[derive(Clone, PartialEq)]
enum FormStatus {
    Idle,
    Pending(String),
    Done(String),
}

/// Render an assistant message that may contain `adi-form` blocks: plain Markdown for the prose,
/// a live form for each valid block. When `forms` is false (a user turn, or a non-conversation
/// backend) the whole text renders as Markdown, so a form block simply shows as a code block.
pub(crate) fn render_message(state: State, text: &str, forms: bool) -> AnyView {
    if !forms || !text.contains(FORM_FENCE) {
        return crate::markdown::render(text);
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<AnyView> = Vec::new();
    let mut md = String::new();
    let mut i = 0;
    while i < lines.len() {
        if is_form_open(lines[i]) {
            // Gather up to the closing fence; parse the body as an AgentForm.
            let mut j = i + 1;
            while j < lines.len() && !is_fence_close(lines[j]) {
                j += 1;
            }
            let closed = j < lines.len();
            if closed {
                let body = lines[i + 1..j].join("\n");
                if let Ok(spec) = serde_json::from_str::<AgentForm>(body.trim()) {
                    flush_md(&mut md, &mut out);
                    out.push(agent_form_view(state, spec));
                    i = j + 1;
                    continue;
                }
            }
            // Not a valid form block: keep the whole thing verbatim so Markdown renders it as a
            // code block (never silently dropped). Consume through the close fence when there was one.
            let last = if closed { j } else { lines.len() - 1 };
            for line in &lines[i..=last] {
                md.push_str(line);
                md.push('\n');
            }
            i = last + 1;
            continue;
        }
        md.push_str(lines[i]);
        md.push('\n');
        i += 1;
    }
    flush_md(&mut md, &mut out);
    out.into_any()
}

/// Flush the accumulated Markdown run (if any) as one rendered block.
fn flush_md(md: &mut String, out: &mut Vec<AnyView>) {
    if !md.trim().is_empty() {
        out.push(crate::markdown::render(md));
    }
    md.clear();
}

/// Whether `line` opens an `adi-form` fence (```` ```adi-form ````).
fn is_form_open(line: &str) -> bool {
    line.trim()
        .strip_prefix("```")
        .is_some_and(|info| info.trim() == FORM_FENCE)
}

/// Whether `line` is a bare closing fence (a run of at least three backticks).
fn is_fence_close(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.bytes().all(|b| b == b'`')
}

/// Render one parsed form: a titled card of prefilled fields over its submit button / status line.
fn agent_form_view(state: State, spec: AgentForm) -> AnyView {
    let AgentForm { title, description, submit_label, action, fields } = spec;
    let label = if submit_label.trim().is_empty() {
        default_submit_label(&action)
    } else {
        submit_label
    };
    // One value signal per field, seeded with the agent's prefill.
    let sigs: BTreeMap<String, RwSignal<String>> = fields
        .iter()
        .map(|f| (f.name.clone(), RwSignal::new(f.value.clone())))
        .collect();
    let field_views = fields
        .iter()
        .map(|f| field_view(f, sigs[&f.name]))
        .collect::<Vec<_>>();
    let status = RwSignal::new(FormStatus::Idle);
    let name_sig = sigs.get("name").copied();
    let project_sig = sigs.get("project").copied();

    // Load the secrets list once so the footer can tell "already stored" from "not yet" — the key
    // to a refresh showing ✓ for a secret that was already created, instead of the button again.
    if state.secrets.get_untracked().is_none() {
        spawn_local(async move {
            if let Ok(sec) = fetch::secrets().await {
                state.secrets.set(Some(sec));
            }
        });
    }

    // An OAuth secret is authorized in another tab; listen for that tab reporting it was stored and
    // flip this form to ✓ live. Matched by secret name.
    if matches!(action, AgentFormAction::OauthSecret { .. }) {
        install_oauth_done_listener(status, name_sig);
    }

    let sigs_submit = sigs.clone();
    let submit_action = action.clone();
    view! {
        <form class="adi-form adi-chatform"
            on:submit=move |ev| {
                ev.prevent_default();
                submit(state, &submit_action, &sigs_submit, status);
            }>
            <div class="adi-chatform__title">{title}</div>
            {(!description.is_empty()).then(move || view! { <p class="adi-hint">{description}</p> })}
            {field_views}
            {move || footer(state, status.get(), name_sig, project_sig, label.clone())}
        </form>
    }
    .into_any()
}

/// The form footer, reactive over three sources: a live action's `status`, and — while idle — whether
/// the secrets list already holds this form's secret (so a refresh shows ✓, not the button again).
fn footer(
    state: State,
    status: FormStatus,
    name_sig: Option<RwSignal<String>>,
    project_sig: Option<RwSignal<String>>,
    label: String,
) -> AnyView {
    match status {
        FormStatus::Pending(msg) => pending_view(msg),
        FormStatus::Done(msg) => done_view(msg),
        FormStatus::Idle => match existing_secret_name(state, name_sig, project_sig) {
            Some(existing) => done_view(format!("Already stored \u{201c}{existing}\u{201d}.")),
            None => view! {
                <button class="adi-btn adi-btn--primary" type="submit">{label}</button>
            }
            .into_any(),
        },
    }
}

/// The name of the secret this form targets if it already exists in the loaded secrets list (matched
/// on the current name + project fields), else `None` — used to render "already stored".
fn existing_secret_name(
    state: State,
    name_sig: Option<RwSignal<String>>,
    project_sig: Option<RwSignal<String>>,
) -> Option<String> {
    let name = name_sig.map(|s| s.get()).unwrap_or_default();
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let project = project_sig.map(|s| s.get()).unwrap_or_default();
    let project = project.trim();
    let want_project = (!project.is_empty()).then_some(project);
    let secrets = state.secrets.get()?;
    secrets
        .secrets
        .iter()
        .find(|s| s.name == name && s.project.as_deref() == want_project)
        .map(|s| s.name.clone())
}

/// A pending status line (an action is in flight, or waiting on the OAuth tab).
fn pending_view(msg: String) -> AnyView {
    view! { <p class="adi-chatform__status">"\u{2026} "{msg}</p> }.into_any()
}

/// A terminal ✓ line.
fn done_view(msg: String) -> AnyView {
    view! { <p class="adi-chatform__done">"\u{2713} "{msg}</p> }.into_any()
}

/// The default submit-button label for an action, used when the form spec gives none.
fn default_submit_label(action: &AgentFormAction) -> String {
    match action {
        AgentFormAction::SetSecret => "Save secret".to_string(),
        AgentFormAction::OauthSecret { provider, .. } => {
            format!("Authorize with {}", provider_label(provider))
        }
    }
}

/// Listen for the OAuth tab's cross-tab "done" marker and flip `status` to ✓ when it names this
/// form's secret. The `storage` event fires in *other* tabs on a localStorage write, so the tab
/// that launched the form hears the tab that finished it. The listener is leaked deliberately (as
/// the app's popstate listener is): `try_get_untracked` / `try_set` make its body a no-op once the
/// form has unmounted (its signals disposed), and OAuth forms are rare, so retaining it is cheap.
fn install_oauth_done_listener(status: RwSignal<FormStatus>, name_sig: Option<RwSignal<String>>) {
    let closure = Closure::<dyn FnMut()>::new(move || {
        let Some(done) = read_oauth_done() else { return };
        let want = name_sig.and_then(|s| s.try_get_untracked()).unwrap_or_default();
        if done == want.trim() {
            clear_oauth_done();
            let _ = status
                .try_set(FormStatus::Done(format!("Authorized & stored \u{201c}{done}\u{201d}.")));
        }
    });
    if let Some(win) = web_sys::window() {
        let _ = win.add_event_listener_with_callback("storage", closure.as_ref().unchecked_ref());
    }
    closure.forget();
}

/// Render one field control, bound to its value signal.
fn field_view(f: &AgentFormInput, sig: RwSignal<String>) -> AnyView {
    let label = f.label.clone();
    let hint = f.hint.clone();

    // A checkbox reads as a single labelled line, so it gets its own layout.
    if matches!(f.kind, AgentFormInputKind::Checkbox) {
        return view! {
            <label class="adi-field" style="display:flex; gap:var(--space-2); align-items:center">
                <input type="checkbox"
                    prop:checked=move || sig.get() == "true"
                    on:change=move |ev| sig.set(
                        if event_target_checked(&ev) { "true" } else { "false" }.to_string()
                    ) />
                <span>{label}</span>
                {(!hint.is_empty()).then(|| field_hint(hint))}
            </label>
        }
        .into_any();
    }

    let control = match f.kind {
        AgentFormInputKind::Textarea => {
            let ph = f.placeholder.clone();
            view! {
                <textarea class="adi-textarea" autocomplete="off" placeholder=ph
                    prop:value=move || sig.get()
                    on:input=move |ev| sig.set(event_target_value(&ev))></textarea>
            }
            .into_any()
        }
        AgentFormInputKind::Select => {
            let opts = f
                .options
                .iter()
                .map(|o| {
                    let value = o.value.clone();
                    view! { <option value=value>{o.label.clone()}</option> }
                })
                .collect::<Vec<_>>();
            view! {
                <select class="adi-input"
                    prop:value=move || sig.get()
                    on:change=move |ev| sig.set(event_target_value(&ev))>
                    {opts}
                </select>
            }
            .into_any()
        }
        kind => {
            // Text / Number / Secret → an <input>, differing only in type + inputmode.
            let input_type = if matches!(kind, AgentFormInputKind::Secret) {
                "password"
            } else {
                "text"
            };
            let inputmode = if matches!(kind, AgentFormInputKind::Number) {
                "numeric"
            } else {
                "text"
            };
            let mut class = String::from("adi-input adi-input--wide");
            if f.mono {
                class.push_str(" adi-mono");
            }
            let ph = f.placeholder.clone();
            view! {
                <input class=class type=input_type inputmode=inputmode autocomplete="off" placeholder=ph
                    prop:value=move || sig.get()
                    on:input=move |ev| sig.set(event_target_value(&ev)) />
            }
            .into_any()
        }
    };

    // Plain `.adi-field` (no `--grow`): in this vertical card `flex:1` would stretch each field to
    // its 240px basis in the *column* direction, leaving tall gaps. The grid already gives a
    // full-width input via `align-items: stretch` on the card.
    view! {
        <div class="adi-field">
            <label class="adi-field__label">{label}</label>
            {control}
            {(!hint.is_empty()).then(|| field_hint(hint))}
        </div>
    }
    .into_any()
}

/// Run the form's action from the current field values, driving `status`. Empty `name` is rejected
/// with a flash. A text secret is set in place (Saving… → ✓); an OAuth secret opens the provider in
/// a new tab (Pending), and the cross-tab listener finishes it.
fn submit(
    state: State,
    action: &AgentFormAction,
    sigs: &BTreeMap<String, RwSignal<String>>,
    status: RwSignal<FormStatus>,
) {
    let raw = |k: &str| sigs.get(k).map(|s| s.get()).unwrap_or_default();
    let opt = |k: &str| {
        let v = raw(k).trim().to_string();
        (!v.is_empty()).then_some(v)
    };
    let name = raw("name").trim().to_string();
    if name.is_empty() {
        state.flash.set(Some(Flash::err("A secret name is required.".to_string())));
        return;
    }
    let project = opt("project");
    let description = opt("description");
    match action {
        // The value keeps its exact bytes (no trim) — a secret may end in whitespace by design.
        AgentFormAction::SetSecret => {
            let body = SetSecret { project, name: name.clone(), value: raw("value"), description };
            status.set(FormStatus::Pending("Saving\u{2026}".to_string()));
            spawn_local(async move {
                match fetch::set_secret(body).await {
                    Ok(sec) => {
                        state.secrets.set(Some(sec));
                        state.flash.set(Some(Flash::ok(format!("Set secret \u{201c}{name}\u{201d}."))));
                        status.set(FormStatus::Done(format!("Saved \u{201c}{name}\u{201d}.")));
                    }
                    Err(e) => {
                        state.flash.set(Some(Flash::err(e)));
                        status.set(FormStatus::Idle);
                    }
                }
            });
        }
        AgentFormAction::OauthSecret { provider, scopes } => {
            let scope = (!scopes.is_empty()).then(|| scopes.join(" "));
            begin_oauth_secret(name, project, description, provider.clone(), scope);
            status.set(FormStatus::Pending(
                "Opened a new tab \u{2014} finish sign-in there; this updates when the secret is stored."
                    .to_string(),
            ));
        }
    }
}
