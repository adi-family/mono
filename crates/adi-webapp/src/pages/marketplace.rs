//! The Marketplace page: apps from the manifests this machine trusts, listed from the cache and
//! installed as git clones pinned to a commit.
//!
//! Grouped by marketplace — the grouping *is* the information, because which manifest an app came
//! from is the trust question a reader is asking when they look at one — with each source's URL
//! and freshness said out loud. A stale source says it is stale and why, the same sentence the
//! CLI prints, rather than looking current.
//!
//! **No install counts anywhere on this page.** Under the standing decision an install does not
//! count toward anything, and a number beside the apps would invite the wrong story at any size.
//!
//! Three things the page makes visible that the first version of it did not:
//!
//! * **You name your copy.** Install opens a form with the entry's own name in it and a note that
//!   it can be renamed later — because the name becomes the dashboard, its id and its hostname,
//!   and a machine-chosen one is how a person ends up with a dashboard they cannot find.
//! * **What installs is a repository at a commit.** Both are on the row, in mono, so "what am I
//!   about to run" has an answer that does not require trusting the listing text.
//! * **A copy that is behind says so**, and Update moves it onto the pin the manifest carries now.
//!
//! Installed and running stay different states on purpose: the app's backend is somebody else's
//! TypeScript, and running it is a choice somebody makes.

use adi_ui::{Icon, IconSize, Lucide};
use adi_webapp_api::types::{MarketplaceApp, MarketplaceInstall, MarketplaceSource};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, MarketplaceForm, State};
use crate::ui::{TextField, confirm, field_hint, flash_view};

/// The Marketplace page: a line on what the page does with Sync beside it, then one section per
/// source with one row per app.
pub(crate) fn marketplace_view(state: State, form: MarketplaceForm) -> AnyView {
    view! {
        <div class="adi-market__lead">
            <span>
                "An app is a git repository at a pinned commit. Installing clones it under a name \
                 you choose; nothing runs until you start it."
            </span>
            <span class="adi-spacer"></span>
            {sync_button(state, form)}
        </div>
        {flash_view(state.flash)}

        {source_panels(state, form)}
    }
    .into_any()
}

/// The Sync button — the one control on the page that leaves the machine, which is why it is a
/// button and not a poll. Busy while it runs; a manifest can sit behind a slow host.
fn sync_button(state: State, form: MarketplaceForm) -> AnyView {
    let busy = form.busy;
    view! {
        <button class="adi-btn" type="button"
            prop:disabled=move || busy.get().as_deref() == Some(SYNC_KEY)
            on:click=move |_| {
                busy.set(Some(SYNC_KEY.to_string()));
                spawn_local(async move {
                    match fetch::sync_marketplace().await {
                        Ok(done) => {
                            state.marketplace.set(Some(done.state));
                            state.flash.set(Some(Flash::ok(done.message)));
                        }
                        Err(e) => state.flash.set(Some(Flash::err(e))),
                    }
                    busy.set(None);
                });
            }>
            <Icon icon=Lucide::RefreshCw/>
            {move || if busy.get().as_deref() == Some(SYNC_KEY) { "Syncing\u{2026}" } else { "Sync" }}
        </button>
    }
    .into_any()
}

/// The busy key of the page's one shared action.
const SYNC_KEY: &str = "sync";

/// One section per marketplace, in the order the sources were added. A store with no sources says
/// how to add one rather than rendering nothing — the CLI is the door for that act, and the page
/// names it.
fn source_panels(state: State, form: MarketplaceForm) -> AnyView {
    view! {
        {move || {
            let Some(loaded) = state.marketplace.get() else {
                return view! { <div class="adi-empty">"Loading\u{2026}"</div> }.into_any();
            };
            if loaded.sources.is_empty() {
                return view! {
                    <p class="adi-hint">
                        "No marketplaces configured. Add one from a shell with "
                        <code>"adi-mono marketplace add <name> <https://manifest-url>"</code>
                        ", then Sync here."
                    </p>
                }.into_any();
            }
            loaded.sources.iter().map(|source| {
                let apps: Vec<MarketplaceApp> = loaded.apps.iter()
                    .filter(|app| app.marketplace == source.name)
                    .cloned()
                    .collect();
                source_panel(state, form, source, &apps)
            }).collect::<Vec<_>>().into_any()
        }}
    }
    .into_any()
}

/// One marketplace's section: its name, where it points, whether what it shows is fresh — then
/// one row per app it lists.
fn source_panel(
    state: State,
    form: MarketplaceForm,
    source: &MarketplaceSource,
    apps: &[MarketplaceApp],
) -> AnyView {
    let (name, url, freshness) = (
        source.name.clone(),
        source.url.clone(),
        freshness_note(source),
    );
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{name.clone()}</h2>
                <span class="adi-mono adi-muted adi-market__url" title=url.clone()>{url.clone()}</span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{freshness}</span>
            </div>
            {if apps.is_empty() {
                view! { <div class="adi-empty">"Nothing in this manifest yet."</div> }.into_any()
            } else {
                apps.iter()
                    .map(|app| app_entry(state, form, app))
                    .collect::<Vec<_>>()
                    .into_any()
            }}
        </section>
    }
    .into_any()
}

/// The sentence beside a source's name: when it last synced, and — when the fetch since has
/// failed — that what is shown is the stale copy, and why. The same facts the CLI's `list`
/// prints, so the two doors never disagree.
fn freshness_note(source: &MarketplaceSource) -> String {
    match (source.synced_at, source.error.as_deref()) {
        (None, _) => "never synced".to_string(),
        (Some(_), Some(error)) => format!("stale — the last sync failed: {error}"),
        (Some(at), None) => format!("synced {}", ago(at)),
    }
}

/// One entry: the row itself, the install form when it is open on this entry, and a line per copy
/// already installed here.
fn app_entry(state: State, form: MarketplaceForm, app: &MarketplaceApp) -> AnyView {
    let key = format!("{}/{}", app.marketplace, app.slug);
    let (row_key, form_key) = (key.clone(), key.clone());
    let owned = app.clone();
    view! {
        <div class="adi-market__entry">
            {app_row(form, &owned, row_key)}
            {move || {
                (form.installing.get() == form_key).then(|| install_form(state, form, &owned))
            }}
            {app.installs.iter()
                .map(|install| copy_row(state, form, install))
                .collect::<Vec<_>>()}
        </div>
    }
    .into_any()
}

/// The entry itself: name, version and one-liner on the left; what it installs from underneath, in
/// mono; the button that opens the install form on the right.
///
/// The repository and the commit are on the row rather than behind a disclosure because they are
/// the whole answer to "whose code is this, and which version of it" — the question the listing
/// text cannot answer for you.
fn app_row(form: MarketplaceForm, app: &MarketplaceApp, key: String) -> AnyView {
    let busy = form.busy;
    let (name, version, description) = (
        app.name.clone(),
        app.version.clone(),
        app.description.clone(),
    );
    let (repo, commit) = (app.repo.clone(), short_commit(&app.commit));
    // The full strings, for the `title` of the elements that show them elided.
    let (repo_title, key_title) = (repo.clone(), key.clone());
    let again = !app.installs.is_empty();
    let open_key = key.clone();
    let default_name = app.name.clone();

    view! {
        <div class="adi-market__row">
            <div class="adi-market__about">
                <div class="adi-market__title">
                    <span class="adi-market__name">{name}</span>
                    {version.map(|v| view! { <span class="adi-mono adi-muted">{v}</span> })}
                </div>
                {description.map(|d| view! { <div class="adi-market__desc">{d}</div> })}
                <div class="adi-mono adi-muted adi-market__origin" title=repo_title>
                    {repo}" @ "{commit}
                </div>
                <div class="adi-mono adi-muted" title=key_title>{key}</div>
            </div>
            <div class="adi-market__actions">
                <button class="adi-btn" type="button"
                    prop:disabled=move || busy.get().is_some()
                    on:click=move |_| {
                        // Prefilled with the publisher's name, because it is the answer most
                        // people want and the form is here to let them disagree with it.
                        form.name.set(default_name.clone());
                        // Starting is on by default *here* and off in the CLI, and the difference
                        // is not an inconsistency: pressing Install on a page is the deliberate
                        // act, and an install that leaves nothing to open reads as one that did
                        // not happen — an unstarted app is filed under Archived on the Dashboards
                        // page, which is the last place anybody goes looking for what they just
                        // installed. Unticking it is one click for whoever wants it inert.
                        form.start_now.set(true);
                        form.installing.update(|open| {
                            *open = if *open == open_key { String::new() }
                                    else { open_key.clone() };
                        });
                    }>
                    {if again { "Install another" } else { "Install" }}
                </button>
            </div>
        </div>
    }
    .into_any()
}

/// The one question an install has to ask: what to call this copy.
///
/// The name becomes the dashboard's name, its id (`Sales CRM` → `sales-crm`) and its hostname, so
/// it is worth a form rather than a guess — and the hint says out loud that it is renameable,
/// which is what makes the form cheap to answer rather than a decision to agonise over.
fn install_form(state: State, form: MarketplaceForm, app: &MarketplaceApp) -> AnyView {
    let key = format!("{}/{}", app.marketplace, app.slug);
    let busy = form.busy;
    let (marketplace, slug) = (app.marketplace.clone(), app.slug.clone());
    view! {
        <div class="adi-market__install">
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let (name, start) = (form.name.get().trim().to_string(), form.start_now.get());
                let (marketplace, slug) = (marketplace.clone(), slug.clone());
                form.installing.set(String::new());
                run(
                    state,
                    form,
                    key.clone(),
                    fetch::install_marketplace_app(marketplace, slug, name, start),
                );
            }>
                <TextField id="market-name" label="Name it" placeholder="Sales CRM"
                    value=form.name />
                <label class="adi-field adi-field--check">
                    <input type="checkbox"
                        prop:checked=move || form.start_now.get()
                        on:change=move |ev| form.start_now.set(event_target_checked(&ev)) />
                    <span class="adi-field__label">"Start it right away"</span>
                    {field_hint("this runs the app's own code on this machine")}
                </label>
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || busy.get().is_some()>
                    "Install"
                </button>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| form.installing.set(String::new())>
                    "Cancel"
                </button>
            </form>
            // Said out loud rather than behind a hint marker: the whole reason the form is worth
            // filling in is that the answer is cheap, and nobody knows that until they are told.
            <p class="adi-hint">
                "This is what you will see it under \u{2014} it becomes the dashboard's name and \
                 its address, and you can rename it later."
            </p>
        </div>
    }
    .into_any()
}

/// One installed copy: what it is called, where it stands, and the one or two acts it allows.
///
/// A copy that is behind the manifest's pin says so and offers Update — a fast-forward, so an
/// operator's own commits on top of the app are never walked over. Force is offered beside it
/// because the refusal is otherwise a dead end inside the panel, and it is gated by a confirm
/// that says what it costs.
fn copy_row(state: State, form: MarketplaceForm, install: &MarketplaceInstall) -> AnyView {
    let busy = form.busy;
    let key = format!("install:{}", install.id);
    let (id, name, commit) = (
        install.id.clone(),
        install.name.clone(),
        short_commit(&install.commit),
    );
    let (started, outdated) = (install.started, install.outdated);
    let host = install.host.clone();

    let start_key = key.clone();
    let update_key = key.clone();
    let force_key = key.clone();
    let (start_id, update_id, force_id) = (id.clone(), id.clone(), id.clone());

    view! {
        <div class="adi-market__copy">
            <span class="adi-market__copy-name">{name}</span>
            <span class="adi-mono adi-muted">{id.clone()}" @ "{commit}</span>
            {outdated.then(|| view! {
                <span class="adi-market__state">"an update is waiting"</span>
            })}
            <span class="adi-spacer"></span>
            {outdated.then(move || view! {
                <button class="adi-btn" type="button"
                    prop:disabled=move || busy.get().is_some()
                    on:click=move |_| {
                        run(state, form, update_key.clone(),
                            fetch::update_marketplace_app(update_id.clone(), false));
                    }>
                    "Update"
                </button>
                <button class="adi-btn adi-btn--ghost" type="button"
                    prop:disabled=move || busy.get().is_some()
                    on:click=move |_| {
                        if confirm("Reset this copy onto the marketplace's commit? Any changes \
                                    you made to it here are lost.") {
                            run(state, form, force_key.clone(),
                                fetch::update_marketplace_app(force_id.clone(), true));
                        }
                    }>
                    "Force"
                </button>
            })}
            {if started {
                open_link(host.as_deref())
            } else {
                view! {
                    <span class="adi-market__state">"not running"</span>
                    <button class="adi-btn" type="button"
                        prop:disabled=move || busy.get().is_some()
                        on:click=move |_| {
                            run(state, form, start_key.clone(),
                                fetch::start_marketplace_app(start_id.clone()));
                        }>
                        "Start"
                    </button>
                }
                .into_any()
            }}
        </div>
    }
    .into_any()
}

/// A running copy's way out: a link to the host it answers on. Over the mesh the same host is the
/// node's name for it, which is exactly why the link goes through `service_url` rather than being
/// built here.
fn open_link(host: Option<&str>) -> AnyView {
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return view! { <span class="adi-market__state">"running, no routable name"</span> }
            .into_any();
    };
    let host = host.to_string();
    match crate::origin::service_url(&host) {
        Some(href) => view! {
            <span class="adi-market__state">{format!("running at {host}")}</span>
            <a class="adi-btn" href=href.clone() target="_blank" rel="noreferrer" title=href>
                "Open"
                <Icon icon=Lucide::ArrowUpRight size=IconSize::Sm/>
            </a>
        }
        .into_any(),
        None => view! { <span class="adi-market__state">{format!("running at {host}")}</span> }
            .into_any(),
    }
}

/// A commit as it is read out loud: the first seven characters, the way git prints one.
fn short_commit(commit: &str) -> String {
    commit.chars().take(7).collect()
}

/// Run one marketplace action: mark the row busy while it is in flight, fold the returned state
/// in, and flash the server's own sentence — the one the CLI prints — on success.
fn run(
    state: State,
    form: MarketplaceForm,
    key: String,
    fut: impl std::future::Future<Output = Result<adi_webapp_api::types::MarketplaceDone, String>>
    + 'static,
) {
    form.busy.set(Some(key));
    spawn_local(async move {
        match fut.await {
            Ok(done) => {
                state.marketplace.set(Some(done.state));
                state.flash.set(Some(Flash::ok(done.message)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        form.busy.set(None);
    });
}

/// Unix seconds as a person reads the gap: `just now`, `5m ago`, `3h ago`, `4d ago`.
fn ago(at: u64) -> String {
    ago_between(at, now_unix())
}

/// Now, in Unix seconds — the browser's clock on wasm, the system clock anywhere else (which is
/// to say: in this crate's native unit tests, where a wasm import would panic).
#[cfg(target_arch = "wasm32")]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn now_unix() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

#[cfg(not(target_arch = "wasm32"))]
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The gap between two Unix timestamps as a person reads it. Split out from [`ago`] so the
/// formatting is testable without a clock.
fn ago_between(at: u64, now: u64) -> String {
    match now.saturating_sub(at) {
        0..=59 => "just now".to_string(),
        secs if secs < 3600 => format!("{}m ago", secs / 60),
        secs if secs < 86_400 => format!("{}h ago", secs / 3600),
        secs => format!("{}d ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(synced_at: Option<u64>, error: Option<&str>) -> MarketplaceSource {
        MarketplaceSource {
            name: "adi".to_string(),
            url: "https://example/m.json".to_string(),
            synced_at,
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn freshness_says_when_and_never_hides_a_failure() {
        assert_eq!(freshness_note(&source(None, None)), "never synced");
        // The fresh branch's exact wording is the clock's business (covered below); what this
        // pins is that a synced source says it synced, in ago form.
        let fresh = freshness_note(&source(Some(now_unix().saturating_sub(90)), None));
        assert!(
            fresh.starts_with("synced ") && fresh.contains("ago"),
            "{fresh}"
        );
        // The stale branch carries the failure verbatim rather than looking current.
        let note = freshness_note(&source(Some(1_788_288_916), Some("dns went away")));
        assert!(
            note.starts_with("stale — the last sync failed: dns went away"),
            "{note}"
        );
    }

    #[test]
    fn a_commit_is_shown_the_way_git_prints_one() {
        assert_eq!(short_commit("9f2c1d4e5a6b7c8d9e0f1a2b3c4d5e6f70819a2b"), "9f2c1d4");
        assert_eq!(short_commit(""), "", "an absent pin shows as nothing, not as a panic");
    }

    #[test]
    fn ago_reads_as_a_person_reads_a_gap() {
        let now = 1_788_288_916;
        assert_eq!(ago_between(now, now), "just now");
        assert_eq!(ago_between(now - 5 * 60, now), "5m ago");
        assert_eq!(ago_between(now - 3 * 3600, now), "3h ago");
        assert_eq!(ago_between(now - 4 * 86_400, now), "4d ago");
        // A clock behind ours saturates rather than wrapping.
        assert_eq!(ago_between(now + 600, now), "just now");
    }
}
