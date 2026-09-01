//! The root onboarding wizard (`/`): the guided first run that stands the root agent up, and the
//! same form again behind the chat bar's "reconfigure agent".
//!
//! The wizard asks as little as it can. A **setup preset** — served by the API beside the form
//! schema — names a backend, pins the arguments that choice implies, and leaves only the real
//! questions: which model, and the credential it cannot run without. Two named routes in (the
//! Claude login most people already have, an API key anyone can paste), then *Manual*, which pins
//! nothing and hands over the whole schema, prefilled.
//!
//! Every field here is rendered by the Agents page's own renderers from the same server schema, so
//! a knob added to a backend shows up in both places or neither.

use std::collections::BTreeSet;

use adi_ui::Lang;
use adi_webapp_api::types::{
    AgentBackendOption, AgentDto, AgentFormSpec, AgentSetupPreset, AgentSetupSecret, MetaState,
    SaveAgent, SecretRef, SetSecret,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{AgentsForm, State};

use super::agents::{
    agent_argument_values, agent_environment_fields, agent_param_applies, agent_schema_fields,
    load_agent_into_form, parsed_env_vars, parsed_path_dirs, set_agent_field_value,
};
use super::meta_bin_tools;

/// The onboarding steps, in order. Only step 1 is interactive today; the rest scaffold the
/// wizard so "step 1 of N" reads true and there is somewhere to grow into.
const ONBOARDING_STEPS: [&str; 2] = ["Set up your primary agent", "You're ready"];

/// The schema fields the manual form never renders from the schema: the root agent's name is
/// well-known and it is global by definition; its runtime is the wizard's own select (the one
/// carrying "help me to choose?"), and its system prompt its own editor further down the card.
const MANUAL_SKIP: [&str; 4] = ["name", "backend", "project", "system_prompt"];

/// One "do you have…?" branch in the runtime picker: the situation, a note on the shared
/// credential, and the runtimes that fit it. Each option is an `(id, how)` pair — the backend id
/// (matched against the server form spec for its label) and a short note on how that runtime runs.
struct RuntimeGuide {
    question: &'static str,
    note: &'static str,
    options: &'static [(&'static str, &'static str)],
}

/// The "help me to choose?" guidance behind the *Manual* preset's runtime select: a short "do you
/// have…?" checklist that maps what a user already has onto one or more runtimes. Every id matches
/// a backend in the server's form spec, so picking one drops straight into the select. The two
/// common answers are presets of their own now, so this is what is left: the runtimes no preset
/// covers, and the CLIs someone may prefer to the headless loop.
const RUNTIME_GUIDE: [RuntimeGuide; 4] = [
    RuntimeGuide {
        question: "I have a Claude subscription (Pro / Max), or the Claude CLI already logged in",
        note: "Uses your existing Claude login — no API key needed. Same credentials either way:",
        options: &[
            ("pty:claude", "Claude Code in a live terminal session"),
            (
                "harness:claude-sdk",
                "the Claude SDK, headless, on the same login",
            ),
        ],
    },
    RuntimeGuide {
        question: "I have a ChatGPT / Codex subscription",
        note: "Uses your Codex login.",
        options: &[("pty:codex", "the Codex CLI in a live terminal session")],
    },
    RuntimeGuide {
        question: "I have an Anthropic API key",
        note: "Talks to the Claude API directly with your key — no CLI login required.",
        options: &[(
            "harness:adi",
            "the ADI agent loop, on the Anthropic provider",
        )],
    },
    RuntimeGuide {
        question: "I have another provider's API key (OpenAI, Gemini, Kimi, GLM, …)",
        note: "ADI's built-in agent loop speaks to many providers with your key.",
        options: &[("harness:adi", "the ADI agent loop")],
    },
];

/// Everything the wizard edits. The agent itself rides in the same [`AgentsForm`] the full Agents
/// page uses — that is what lets the manual preset be the real form rather than a copy of it —
/// with the wizard's own state around it: which preset is chosen, the API key typed into it (held
/// apart from the agent, because it is stored as a secret and never as an argument), and the two
/// disclosures. `Copy`, so it threads into handlers.
#[derive(Clone, Copy)]
pub(crate) struct OnboardingForm {
    pub(crate) agent: AgentsForm,
    /// The chosen preset id, empty until the first one is applied.
    preset: RwSignal<String>,
    /// The plaintext API key typed into the preset's key field, stored as a secret on submit.
    key: RwSignal<String>,
    error: RwSignal<Option<String>>,
    /// True while editing an agent that already exists, so the wizard shows in place of the chat.
    pub(crate) reconfiguring: RwSignal<bool>,
    /// Whether the manual preset's "help me to choose?" runtime modal is open.
    show_help: RwSignal<bool>,
    /// The system prompt is advanced and seeded from a sensible default, so it starts collapsed.
    show_prompt: RwSignal<bool>,
}

impl OnboardingForm {
    pub(crate) fn new() -> Self {
        Self {
            agent: AgentsForm::new(),
            preset: RwSignal::new(String::new()),
            key: RwSignal::new(String::new()),
            error: RwSignal::new(None),
            reconfiguring: RwSignal::new(false),
            show_help: RwSignal::new(false),
            show_prompt: RwSignal::new(false),
        }
    }
}

/// Seed the wizard from `/api/meta`: the default system prompt for a first run, and the preset the
/// answers should start on. Called once, when meta first lands.
pub(crate) fn seed_onboarding(form: OnboardingForm, m: &MetaState) {
    // An agent already exists: the wizard is only reachable through "reconfigure" from here, so
    // load it and land on whichever preset describes what it already is.
    if let Some(agent) = m.agent.as_ref() {
        load_reconfigure(form, m, agent);
        return;
    }
    if form.agent.system_prompt.get_untracked().is_empty() {
        form.agent.system_prompt.set(m.default_prompt.clone());
    }
    if form.preset.get_untracked().is_empty()
        && let Some(first) = m.form.presets.first()
    {
        apply_preset(form, &m.form.presets, &first.id);
    }
}

/// The chat bar's "reconfigure agent": load the stored agent into the wizard and show it in place
/// of the chat.
pub(crate) fn start_reconfigure(form: OnboardingForm, m: &MetaState) {
    if let Some(agent) = m.agent.as_ref() {
        load_reconfigure(form, m, agent);
        form.reconfiguring.set(true);
    }
}

/// Load an existing agent into the wizard, on the preset that describes it.
fn load_reconfigure(form: OnboardingForm, m: &MetaState, agent: &AgentDto) {
    load_agent_into_form(form.agent, agent);
    form.key.set(String::new());
    form.error.set(None);
    form.preset.set(preset_for(&m.form.presets, agent));
}

/// Which preset an existing agent came from: the first whose backend and **pinned** arguments the
/// agent still matches, else manual. Only the pinned ones identify a preset — an argument it also
/// asks for was a prefill, and an agent that took the Kimi route and then changed its model is
/// still on the Kimi route. A preset that has since changed its mind about a pin reads as manual
/// rather than silently rewriting the agent on the next save.
fn preset_for(presets: &[AgentSetupPreset], agent: &AgentDto) -> String {
    presets
        .iter()
        .find(|p| {
            !p.manual
                && p.backend == agent.backend
                && p.arguments
                    .iter()
                    .filter(|(name, _)| !p.fields.contains(name))
                    .all(|(name, value)| {
                        agent.arguments.get(name).and_then(|v| v.as_str()) == Some(value.as_str())
                    })
        })
        .or_else(|| presets.iter().find(|p| p.manual))
        .map(|p| p.id.clone())
        .unwrap_or_default()
}

/// Point the form at a preset: the backend it saves as, and the arguments it pins or prefills,
/// written into the same signals the user's own typing lands in. The API key box is cleared —
/// it belongs to the preset that asked for it.
fn apply_preset(form: OnboardingForm, presets: &[AgentSetupPreset], id: &str) {
    form.preset.set(id.to_string());
    form.key.set(String::new());
    form.error.set(None);
    let Some(preset) = presets.iter().find(|p| p.id == id) else {
        return;
    };
    // The manual preset names no backend: it asks, and whatever was already chosen is the answer
    // it starts from.
    if !preset.backend.is_empty() {
        form.agent.backend.set(preset.backend.clone());
    }
    for (name, value) in &preset.arguments {
        set_agent_field_value(form.agent, name, value.clone());
    }
}

/// The wizard: the greeting, the stepper, and step 1's setup card.
pub(crate) fn onboarding_view(state: State, form: OnboardingForm, m: MetaState) -> AnyView {
    let first_run = m.agent.is_none();
    view! {
        {onb_intro(first_run)}
        <ol class="adi-onb__steps">{onb_steps(first_run)}</ol>
        {onb_setup_card(state, form, m)}
    }
    .into_any()
}

/// The wizard's heading. "Welcome to adi." is the greeting for an actual first run — no agent
/// exists yet — so a reconfigure (same form, reached from the chat's bar) gets a heading that
/// says where you are instead of greeting someone who has been here all along.
fn onb_intro(first_run: bool) -> AnyView {
    if first_run {
        view! {
            <div class="adi-onb__intro">
                <h1 class="adi-onb__welcome">
                    "Welcome to adi"<span class="adi-onb__dot">"."</span>
                </h1>
                <p class="adi-onb__sub">"Let\u{2019}s set up your primary agent."</p>
            </div>
        }
        .into_any()
    } else {
        view! {
            <div class="adi-onb__intro">
                <h1 class="adi-onb__welcome">"Reconfigure your agent"</h1>
                <p class="adi-onb__sub">
                    "Change the runtime it runs on, its credentials, or its system prompt."
                </p>
            </div>
        }
        .into_any()
    }
}

/// The stepper row: one node per onboarding step. Step 1 is `active` on a first run and `done`
/// once the agent exists (a reconfigure is step 1 again, from the other side); later steps are
/// `upcoming`.
fn onb_steps(first_run: bool) -> Vec<AnyView> {
    ONBOARDING_STEPS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let state = if i > 0 {
                "upcoming"
            } else if first_run {
                "active"
            } else {
                "done"
            };
            let num = if i == 0 && !first_run {
                "\u{2713}".to_string()
            } else {
                (i + 1).to_string()
            };
            view! {
                <li class="adi-onb__step" data-state=state>
                    <span class="adi-onb__step-num">{num}</span>
                    <span class="adi-onb__step-label">{(*label).to_string()}</span>
                </li>
            }
            .into_any()
        })
        .collect()
}

/// Step 1's setup card: the preset picker, the fields that preset asks for, the (collapsed)
/// system prompt, and the save. Doubles as create (no agent yet) and reconfigure (an agent exists
/// and Cancel returns to the chat).
fn onb_setup_card(state: State, form: OnboardingForm, m: MetaState) -> AnyView {
    let creating = m.agent.is_none();
    let presets = m.form.presets.clone();
    let for_submit = m.clone();
    let for_body = m.clone();
    view! {
        <div class="adi-onb__card">
            <span class="adi-onb__eyebrow">"Step 1"</span>
            <h2 class="adi-onb__title">"Set up your primary agent"</h2>
            <p class="adi-onb__desc">
                <strong>"adi-agent"</strong>
                " is your environment's root agent — a meta-agent that helps you set up and
                 operate this ADI stack. Pick how it should run and give it what that needs;
                 every tool in your store is enabled on it. You can change all of it later."
            </p>
            <form class="adi-onb__form" on:submit=move |ev| {
                ev.prevent_default();
                submit_onb_agent(state, form, &for_submit);
            }>
                {preset_picker(form, presets)}
                {move || preset_body(state, form, &for_body)}
                {prompt_disclosure(form)}

                {move || form.error.get().map(|e| view! { <p class="adi-onb__error">{e}</p> })}

                <div class="adi-onb__actions">
                    {(!creating).then(|| view! {
                        <button class="adi-btn adi-btn--link" type="button"
                            on:click=move |_| form.reconfiguring.set(false)>"Cancel"</button>
                    })}
                    <span class="adi-spacer"></span>
                    <button class="adi-btn adi-btn--primary adi-onb__submit" type="submit"
                        prop:disabled=move || form.agent.busy.get()>
                        {move || match (form.agent.busy.get(), creating) {
                            (true, _) => "Saving…",
                            (false, true) => "Create adi-agent",
                            (false, false) => "Save changes",
                        }}
                    </button>
                </div>
            </form>
        </div>

        {move || form.show_help.get()
            .then(|| onb_help_modal(form, m.form.backends.clone()))}
    }
    .into_any()
}

/// The preset picker: one segment per way in, over the chosen one's blurb. A radiogroup rather
/// than a `<select>` — there are three of them and each needs a line of explanation, which a
/// dropdown has nowhere to put.
fn preset_picker(form: OnboardingForm, presets: Vec<AgentSetupPreset>) -> AnyView {
    let blurbs = presets.clone();
    let segments = presets
        .into_iter()
        .map(|preset| {
            let id = preset.id.clone();
            let for_flag = id.clone();
            let for_aria = id.clone();
            let all = blurbs.clone();
            view! {
                <button class="adi-onb__preset" type="button" role="radio"
                    data-on=move || (form.preset.get() == for_flag).to_string()
                    aria-checked=move || (form.preset.get() == for_aria).to_string()
                    on:click=move |_| apply_preset(form, &all, &id)>
                    {preset.label}
                </button>
            }
        })
        .collect::<Vec<_>>();
    let blurb_of = blurbs.clone();
    view! {
        <div class="adi-field">
            <label class="adi-field__label">"How should it run?"</label>
            <div class="adi-onb__presets" role="radiogroup" aria-label="How should it run?">
                {segments}
            </div>
            {move || {
                let on = form.preset.get();
                blurb_of.iter().find(|p| p.id == on).map(|p| view! {
                    <p class="adi-onb__hint">{p.blurb.clone()}</p>
                })
            }}
        </div>
    }
    .into_any()
}

/// What the chosen preset asks for: its own short list of fields plus the API key it needs, or —
/// for the manual preset — the runtime picker and every field that runtime takes.
fn preset_body(state: State, form: OnboardingForm, m: &MetaState) -> AnyView {
    let on = form.preset.get();
    let Some(preset) = m.form.presets.iter().find(|p| p.id == on) else {
        return ().into_any();
    };
    if preset.manual {
        return manual_body(state, form, &m.form);
    }
    let fields = agent_schema_fields(&m.form, Some(&preset.fields), &[], state, form.agent);
    let key = preset.secret.clone().map(|secret| {
        let stored = secret_is_stored(state, &secret.env);
        let id = format!("onb-key-{}", secret.env.to_lowercase().replace('_', "-"));
        let label_for = id.clone();
        let placeholder = if stored {
            "stored — leave blank to keep it".to_string()
        } else {
            secret.placeholder.clone()
        };
        view! {
            <div class="adi-field" style="flex:1 1 100%; min-width:0">
                <label class="adi-field__label" for=label_for>{secret.label.clone()}</label>
                <input class="adi-input adi-mono" id=id type="password" autocomplete="off"
                    spellcheck="false" placeholder=placeholder
                    prop:value=move || form.key.get()
                    on:input=move |ev| form.key.set(event_target_value(&ev)) />
                <p class="adi-onb__hint">
                    {secret.hint.clone()}
                    " Kept as a secret named "
                    <code class="adi-onb__code">{secret.env.clone()}</code>
                    ", attached to this agent alone."
                </p>
            </div>
        }
    });
    view! { <div class="adi-onb__fields">{fields}{key}</div> }.into_any()
}

/// The manual preset: the runtime picker (with the "help me to choose?" way out), then every field
/// that runtime takes, prefilled — the same form the Agents page shows, minus the three questions
/// the root agent has already answered.
fn manual_body(state: State, form: OnboardingForm, spec: &AgentFormSpec) -> AnyView {
    let backends = spec.backends.clone();
    let fields = agent_schema_fields(spec, None, &MANUAL_SKIP, state, form.agent);
    view! {
        <div class="adi-onb__fields">
            <div class="adi-field" style="flex:1 1 100%; min-width:0">
                <div class="adi-onb__field-head">
                    <label class="adi-field__label" for="onb-backend">"Runtime"</label>
                    <button class="adi-onb__help-link" type="button"
                        on:click=move |_| form.show_help.set(true)>"help me to choose?"</button>
                </div>
                <select class="adi-input" id="onb-backend"
                    prop:value=move || form.agent.backend.get()
                    on:change=move |ev| form.agent.backend.set(event_target_value(&ev))>
                    <option value="">"— pick a runtime —"</option>
                    {backends.into_iter().map(|b| view! {
                        <option value=b.id>{b.label}</option>
                    }).collect::<Vec<_>>()}
                </select>
            </div>
            {fields}
            {agent_environment_fields(form.agent)}
        </div>
    }
    .into_any()
}

/// The collapsed "System prompt" row and, once opened, the Markdown editor for it. Shared by every
/// preset: whichever way the agent runs, this is what it is told to be.
fn prompt_disclosure(form: OnboardingForm) -> AnyView {
    let show = form.show_prompt;
    view! {
        <div class="adi-field">
            <button class="adi-onb__disclosure" type="button"
                aria-expanded=move || show.get().to_string()
                on:click=move |_| show.update(|v| *v = !*v)>
                <span class="adi-onb__disclosure-caret"
                    class:is-open=move || show.get()>"\u{25b8}"</span>
                <span class="adi-onb__disclosure-label">"System prompt"</span>
                <span class="adi-onb__disclosure-hint">"optional \u{00b7} advanced"</span>
            </button>
            {move || show.get().then(|| view! {
                <div class="adi-onb__prompt">
                    <p class="adi-onb__hint">
                        "Seeded with a default that orients the agent in your ADI stack and
                         points it at the guides in "
                        <code class="adi-onb__code">"~/.adi/mono/guides"</code>
                        " (dashboards, tasks, tools, …). Edit freely — you can change it
                         later."
                    </p>
                    <adi_ui::CodeEditor value=form.agent.system_prompt lang=Lang::Md
                        height=adi_ui::CodeHeight::Form id="onb-prompt"
                        class="adi-ui-type island"/>
                </div>
            })}
        </div>
    }
    .into_any()
}

/// Whether the store already holds a global secret of this name — the reason a reconfigure never
/// asks for a key that was pasted once already.
fn secret_is_stored(state: State, env: &str) -> bool {
    state.secrets.get().is_some_and(|s| {
        s.secrets
            .iter()
            .any(|secret| secret.project.is_none() && secret.name == env)
    })
}

/// The "help me to choose?" modal: a "do you have…?" checklist (from [`RUNTIME_GUIDE`]) that
/// recommends a runtime for what the user already has and, on "Use this", writes it into the
/// manual preset's select and closes. Clicking the scrim or the ✕ dismisses it.
fn onb_help_modal(form: OnboardingForm, backends: Vec<AgentBackendOption>) -> AnyView {
    let rows = RUNTIME_GUIDE
        .iter()
        .map(|guide| {
            let opts = guide
                .options
                .iter()
                .map(|(id, how)| {
                    let id = (*id).to_string();
                    // Show the server's own label for the runtime when it offers one, so the modal
                    // never drifts from the select; fall back to the raw id if the list lacks it.
                    let label = backends
                        .iter()
                        .find(|b| b.id == id)
                        .map_or_else(|| id.clone(), |b| b.label.clone());
                    let pick = id.clone();
                    view! {
                        <div class="adi-help__opt">
                            <div class="adi-help__opt-main">
                                <code class="adi-onb__code">{label}</code>
                                <span class="adi-help__how">{(*how).to_string()}</span>
                            </div>
                            <span class="adi-spacer"></span>
                            <button class="adi-btn adi-btn--ghost adi-help__use" type="button"
                                on:click=move |_| {
                                    form.agent.backend.set(pick.clone());
                                    form.show_help.set(false);
                                }>"Use this"</button>
                        </div>
                    }
                })
                .collect::<Vec<_>>();
            view! {
                <li class="adi-help__row">
                    <p class="adi-help__q">{guide.question}</p>
                    <p class="adi-help__note">{guide.note}</p>
                    <div class="adi-help__opts">{opts}</div>
                </li>
            }
        })
        .collect::<Vec<_>>();

    view! {
        <div class="adi-help" role="dialog" aria-modal="true" aria-label="Choose a runtime"
            on:click=move |_| form.show_help.set(false)>
            <div class="adi-help__panel" on:click=|ev| ev.stop_propagation()>
                <header class="adi-help__head">
                    <h3 class="adi-help__title">"Which runtime should I pick?"</h3>
                    <button class="adi-btn adi-btn--icon-sm" type="button" aria-label="Close"
                        on:click=move |_| form.show_help.set(false)>"\u{00d7}"</button>
                </header>
                <p class="adi-help__intro">
                    "Tell us what you already have — we\u{2019}ll point you at a matching runtime.
                     You can change it any time."
                </p>
                <ul class="adi-help__list">{rows}</ul>
                <p class="adi-help__foot">
                    "Still unsure? Go back to " <strong>"Claude Code SDK"</strong>
                    " — every runtime is swappable from Extended \u{2192} Meta later."
                </p>
            </div>
        </div>
    }
    .into_any()
}

/// Save the wizard as the `adi-agent` definition (create or update): store the preset's API key as
/// a secret if one was typed, save the agent with that secret attached, then refresh `/api/meta`.
///
/// The key is stored **first**: an agent saved against a variable that holds nothing is an agent
/// whose first run fails for a reason the setup page had in its hand.
fn submit_onb_agent(state: State, form: OnboardingForm, m: &MetaState) {
    let backend = form.agent.backend.get_untracked().trim().to_string();
    if backend.is_empty() {
        form.error
            .set(Some("Pick how the agent should run.".to_string()));
        return;
    }
    let presets = m.form.presets.clone();
    let preset = presets
        .iter()
        .find(|p| p.id == form.preset.get_untracked())
        .cloned();
    let secret = preset.as_ref().and_then(|p| p.secret.clone());
    let key = form.key.get_untracked().trim().to_string();
    if let Some(sec) = secret.as_ref()
        && sec.required
        && key.is_empty()
        && !secret_is_stored(state, &sec.env)
    {
        form.error.set(Some(format!("{} is required.", sec.label)));
        return;
    }

    let provider = form
        .agent
        .argument_values
        .get_untracked()
        .get("provider")
        .cloned()
        .unwrap_or_default();
    let spec = m.form.clone();
    let arguments = agent_argument_values(
        Some(&spec),
        &backend,
        form.agent.arguments.get_untracked(),
        form.agent.argument_values.get_untracked(),
        form.agent,
        agent_param_applies(Some(&spec), &backend, &provider, "permission_mode"),
        agent_param_applies(Some(&spec), &backend, &provider, "temperature"),
    );

    // The root agent is created with every tool the store has (see `meta_bin_tools`).
    let bin_tools = Some(meta_bin_tools(Some(m)));
    let manual = preset.as_ref().is_some_and(|p| p.manual);
    let body = SaveAgent {
        name: m.name.clone(),
        backend,
        arguments,
        // Onboarding creates the agent, so it states its tags, star and secrets outright; `project`
        // stays unstated because there is nothing to keep and an unfiled agent is a global one.
        tags: Some(
            form.agent
                .tags
                .get_untracked()
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
        ),
        starred: Some(form.agent.starred.get_untracked()),
        project: None,
        bin_tools,
        // The wizard offers no pre-run box, and a brand-new agent has nothing to keep: `None` is
        // both "leave it alone" and "there was nothing there".
        prelude: None,
        secrets: Some(attached_secrets(
            state,
            form,
            &presets,
            secret.as_ref(),
            &key,
        )),
        // No form offers the knowledge bases or the memory toggle yet, so none of them
        // states one: `None` leaves whatever the agent already has. Set them with
        // `adi-mono agents save --knowledge … --memory` until the editor grows the
        // checkboxes.
        knowledge: None,
        memory: None,
        // Only the manual preset offers the run environment, so only it states one — `None` leaves
        // whatever the agent already has instead of clearing it on every save.
        path: manual.then(|| parsed_path_dirs(&form.agent.path.get_untracked())),
        env: manual.then(|| parsed_env_vars(&form.agent.env.get_untracked())),
        // Not offered here, so not stated — `None` leaves whatever the agent already has.
        unattended: None,
        rename_from: None,
    };

    form.agent.busy.set(true);
    form.error.set(None);
    let store_key = secret.filter(|_| !key.is_empty()).map(|sec| SetSecret {
        project: None,
        name: sec.env,
        value: key,
        description: Some("Set up on the adi onboarding page.".to_string()),
    });
    spawn_local(async move {
        let mut failed = None;
        if let Some(set) = store_key {
            match fetch::set_secret(set).await {
                Ok(secrets) => state.secrets.set(Some(secrets)),
                Err(e) => failed = Some(e),
            }
        }
        if failed.is_none() {
            match fetch::save_agent(body).await {
                Ok(agents) => {
                    state.agents.set(Some(agents));
                    form.reconfiguring.set(false);
                    if let Ok(m) = fetch::meta().await {
                        state.meta.set(Some(m));
                    }
                }
                Err(e) => failed = Some(e),
            }
        }
        form.error.set(failed);
        form.agent.busy.set(false);
    });
}

/// The secrets to save the agent with. A named preset owns one credential slot: it attaches its
/// own key (once there is one to attach — a variable pointing at nothing is worse than none) and
/// drops the other presets', so switching from a pasted key to a CLI login doesn't leave the agent
/// carrying a variable it no longer reads. The manual preset owns no slot and touches nothing.
fn attached_secrets(
    state: State,
    form: OnboardingForm,
    presets: &[AgentSetupPreset],
    secret: Option<&AgentSetupSecret>,
    key: &str,
) -> Vec<SecretRef> {
    let mut attached: BTreeSet<(Option<String>, String)> = form.agent.secrets.get_untracked();
    if presets
        .iter()
        .find(|p| p.id == form.preset.get_untracked())
        .is_some_and(|p| !p.manual)
    {
        for env in presets.iter().filter_map(|p| p.secret.as_ref()) {
            attached.remove(&(None, env.env.clone()));
        }
        if let Some(sec) = secret
            && (!key.is_empty() || secret_is_stored(state, &sec.env))
        {
            attached.insert((None, sec.env.clone()));
        }
    }
    attached
        .into_iter()
        .map(|(project, name)| SecretRef { project, name })
        .collect()
}
