//! The Marketplace page: apps from the manifests this machine trusts, listed from the cache and
//! installed without starting anything.
//!
//! Grouped by marketplace — the grouping *is* the information, because which manifest an app
//! came from is the trust question a reader is asking when they look at one — with each source's
//! URL and freshness said out loud. A stale source says it is stale and why, the same sentence
//! the CLI prints, rather than looking current.
//!
//! **No install counts anywhere on this page.** Under the standing decision an install does not
//! count toward anything, and a number beside the apps would invite the wrong story at any size.
//!
//! The flow the page carries is deliberately two steps: **Install** lands the app's files —
//! nothing runs — and **Start** is the act that puts it in the supervisor's glob. Installed and
//! running are different states, and the page keeps them different on purpose: the artifact is
//! somebody else's TypeScript, and running it is a choice somebody makes.

use adi_ui::{Icon, IconSize, Lucide};
use adi_webapp_api::types::{MarketplaceApp, MarketplaceSource};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, MarketplaceForm, State};
use crate::ui::flash_view;

/// The Marketplace page: a line on what the page does with Sync beside it, then one section per
/// source with one row per app. Nothing here is orange: installing and starting are choices, not
/// the live state of anything.
pub(crate) fn marketplace_view(state: State, form: MarketplaceForm) -> AnyView {
    view! {
        <div class="adi-market__lead">
            <span>"Apps land installed, not running. Start is the act that runs one."</span>
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
                    .map(|app| app_row(state, form, name.clone(), app))
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

/// One app: its name, version and one-liner on the left; the one action its state allows on the
/// right.
///
/// The three states a row can be in are the page's whole vocabulary: not installed (**Install**),
/// installed and not running (**Start**, with the note that nothing is running yet), and running
/// (**Open**, on the host both of its services claim — through `origin::service_url`, so the link
/// works read over the mesh too). Install is only offered when nothing by that slug is here, so
/// an install never overwrites; replacing one is the CLI's `--force`, deliberately not a button.
fn app_row(
    state: State,
    form: MarketplaceForm,
    marketplace: String,
    app: &MarketplaceApp,
) -> AnyView {
    let key = format!("{marketplace}/{}", app.slug);
    let busy = form.busy;
    let (name, slug, version, description) = (
        app.name.clone(),
        app.slug.clone(),
        app.version.clone(),
        app.description.clone(),
    );

    let action = if app.started {
        open_link(app)
    } else if app.installed {
        let action_key = key.clone();
        view! {
            <span class="adi-market__state">"installed, not running"</span>
            <button class="adi-btn" type="button"
                prop:disabled=move || busy.get().is_some()
                on:click=move |_| {
                    run(state, form, action_key.clone(), fetch::start_marketplace_app(slug.clone()));
                }>
                "Start"
            </button>
        }
        .into_any()
    } else {
        let action_key = key.clone();
        view! {
            <button class="adi-btn" type="button"
                prop:disabled=move || busy.get().is_some()
                on:click=move |_| {
                    run(
                        state,
                        form,
                        action_key.clone(),
                        fetch::install_marketplace_app(marketplace.clone(), slug.clone(), false),
                    );
                }>
                "Install"
            </button>
        }
        .into_any()
    };

    view! {
        <div class="adi-market__row">
            <div class="adi-market__about">
                <div class="adi-market__title">
                    <span class="adi-market__name">{name}</span>
                    {version.map(|v| view! { <span class="adi-mono adi-muted">{v}</span> })}
                </div>
                {description.map(|d| view! { <div class="adi-market__desc">{d}</div> })}
                <div class="adi-mono adi-muted" title=key.clone()>{key.clone()}</div>
            </div>
            <div class="adi-market__actions">{action}</div>
        </div>
    }
    .into_any()
}

/// The running app's way out: a link to the host it answers on. Over the mesh the same host is
/// the node's name for it, which is exactly why the link goes through `service_url` rather than
/// being built here.
fn open_link(app: &MarketplaceApp) -> AnyView {
    let Some(host) = app.host.as_deref().map(str::trim).filter(|h| !h.is_empty()) else {
        return view! { <span class="adi-market__state">"running, no routable name"</span> }
            .into_any();
    };
    match crate::origin::service_url(host) {
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
