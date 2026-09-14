//! The Marketplace: a shelf you browse, and one page per item you act from.
//!
//! The split is the whole shape of this screen. **The listing is for browsing** — a row is one
//! item: its mark, its name, what it is for, what it contains, and whether any of it is here
//! already. Nothing on it installs anything, and nothing on it is a form. **An item's own page is
//! where you act** — the hero's one button installs the lot, and the "What's included" list
//! installs exactly one element of it.
//!
//! That split is what the first version of this page did not have: every row carried its own
//! install button, its own install form, and its own element list, so a marketplace of five items
//! was forty controls and eighty lines of machine strings — a config dump wearing a store's name.
//! A listing whose rows are links and whose actions live one click deeper reads as a shelf, and it
//! also settles DESIGN.md §8's "never repeat an action word per row" for the listing outright.
//!
//! What survived from the first version, because it was right:
//!
//! * **Grouped by marketplace**, with each source's URL and freshness said out loud — which
//!   manifest an item came from is the trust question a reader is asking when they look at one.
//! * **What installs is a repository at a commit**, on the row. Compressed to `host/path @ 9f2c1d4`
//!   rather than the full URL (the whole of it is in the row's `title`, and on the item's page
//!   under Repository), because the fact is load-bearing and its 90 characters were not.
//! * **You name your copy** — for the legacy single-dashboard shape, whose name becomes a
//!   dashboard, an id and a hostname. A general bundle is never asked: every element of one lands
//!   under its own published name, so the form that asks would be a form that lies.
//! * **No install counts anywhere.** Under the standing decision an install does not count toward
//!   anything, and a number beside the items would invite the wrong story at any size.
//!
//! Installed and running stay different states on purpose: the item's code is somebody else's, and
//! running it is a choice somebody makes.

use adi_ui::{Icon, IconSize, Lucide, Markdown, Modal};
use adi_webapp_api::types::{
    MarketplaceApp, MarketplaceBundleElement, MarketplaceBundleInstall, MarketplaceInstall,
    MarketplaceMedia, MarketplaceMediaKind as MediaKind, MarketplaceSource, MarketplaceState,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Destination, Flash, MarketplaceForm, State};
use crate::ui::{TextField, confirm, field_hint, flash_view, menu_item, row_actions};

/// Where a click on an item's name goes.
///
/// `Some` inside the marketplace's own door ([`market_view`]), where an item's page is another URL
/// of the same document and the click is taken over. `None` in the control panel, where it is a
/// plain navigation out to that door — the panel has no page for one item, and pretending it does
/// would be a second implementation of this screen.
type OpenApp = Option<RwSignal<String>>;

/// The marketplace door: the listing, or one item's own page when the URL names one.
///
/// Both are the same document (`main`'s `Market`), so moving between them is a signal and a
/// history entry rather than a load — which is what makes a gallery worth having, since going
/// back to the listing does not cost the wasm bundle a second time.
pub(crate) fn market_view(state: State, form: MarketplaceForm, open: RwSignal<String>) -> AnyView {
    view! {
        {move || {
            let key = open.get();
            if key.is_empty() {
                return listing(state, form, Some(open));
            }
            let Some(loaded) = state.marketplace.get() else {
                return view! { <div class="adi-empty">"Loading\u{2026}"</div> }.into_any();
            };
            match loaded.apps.iter().find(|app| app_key(app) == key) {
                Some(app) => app_page(state, form, app, open),
                // A link to an item the manifest no longer lists — or a slug typed by hand. Say
                // so, and offer the one way on rather than an empty page.
                None => view! {
                    <div class="adi-empty">
                        {format!("Nothing called {key} in the marketplaces this machine follows.")}
                    </div>
                    {back_link(Some(open))}
                }
                .into_any(),
            }
        }}
    }
    .into_any()
}

/// The Marketplace page as the control panel draws it: the listing, and every item's name a link
/// out to the marketplace's own door.
pub(crate) fn marketplace_view(state: State, form: MarketplaceForm) -> AnyView {
    listing(state, form, None)
}

/// The listing itself: a line on what the page is, Sync beside it, then one section per source.
fn listing(state: State, form: MarketplaceForm, open: OpenApp) -> AnyView {
    view! {
        <div class="adi-market__lead">
            <span>
                "Agents, tools, dashboards and backends, published as pinned git repositories. \
                 Open one to see what it carries."
            </span>
            <span class="adi-spacer"></span>
            {sync_button(state, form)}
        </div>
        {flash_view(state.flash)}

        {source_panels(state, open)}
    }
    .into_any()
}

/// How an entry is addressed everywhere: `<marketplace>/<slug>`. The install API's spec, the
/// busy-key of a row, and the item page's URL are all this one string.
fn app_key(app: &MarketplaceApp) -> String {
    format!("{}/{}", app.marketplace, app.slug)
}

/// The Sync button — the one control on the listing that leaves the machine, which is why it is a
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
fn source_panels(state: State, open: OpenApp) -> AnyView {
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
                source_panel(source, &apps, open)
            }).collect::<Vec<_>>().into_any()
        }}
    }
    .into_any()
}

/// One marketplace's section: its name, where it points, whether what it shows is fresh — then one
/// row per item it lists.
fn source_panel(source: &MarketplaceSource, apps: &[MarketplaceApp], open: OpenApp) -> AnyView {
    let (name, url, freshness) = (
        source.name.clone(),
        source.url.clone(),
        freshness_note(source),
    );
    let stale = source.error.is_some();
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{name.clone()}</h2>
                <span class="adi-mono adi-muted adi-market__url" title=url.clone()>{url.clone()}</span>
                <span class="adi-spacer"></span>
                // A stale source is a warning, and a warning is a dot and a sentence — never
                // coloured prose (§3: `--warn` is a 6px dot or a tinted pill, nothing else).
                <span class="adi-updated">
                    {stale.then(|| view! { <span class="adi-dot adi-dot--warn"></span> })}
                    {freshness}
                </span>
            </div>
            {if apps.is_empty() {
                view! { <div class="adi-empty">"Nothing in this manifest yet."</div> }.into_any()
            } else {
                apps.iter()
                    .map(|app| entry_row(app, open))
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

/// One item on the shelf, and the whole row is the way in to it.
///
/// An anchor rather than a row with a link inside it: the target is one item's page, every part of
/// the row is about that item, and a store where only the four words of the name are clickable is a
/// store people think is broken. It is also what lets the row carry a hover tone honestly — nothing
/// here is half-clickable, because nothing here is a control.
fn entry_row(app: &MarketplaceApp, open: OpenApp) -> AnyView {
    let key = app_key(app);
    let (name, version, description) = (
        app.name.clone(),
        app.version.clone(),
        app.description.clone(),
    );
    let (icon, elements) = (app_icon(app), bundle_elements(app));
    let contents = contents_note(&elements);
    let origin = format!("{} @ {}", repo_short(&app.repo), short_commit(&app.commit));
    let origin_title = format!("{} @ {}", app.repo, app.commit);
    let (href, go) = (
        crate::routing::market_app_path(&app.marketplace, &app.slug),
        open_app(open, key),
    );

    view! {
        <a class="adi-market__row" href=href on:click=move |ev| go(&ev)>
            {icon}
            <div class="adi-market__about">
                <div class="adi-market__title">
                    <span class="adi-market__name">{name}</span>
                    {version.map(|v| view! { <span class="adi-mono adi-muted">{v}</span> })}
                </div>
                {description.map(|d| view! { <div class="adi-market__desc">{d}</div> })}
                <div class="adi-market__meta">
                    {(!contents.is_empty()).then(|| view! {
                        <span class="adi-market__contents">{contents}</span>
                    })}
                    <span class="adi-mono adi-market__origin" title=origin_title>{origin}</span>
                </div>
            </div>
            <div class="adi-market__rowstate">{here_note(app, &elements)}</div>
        </a>
    }
    .into_any()
}

/// What this machine already has of an item, as the listing says it — in one short state, because
/// the row is a shelf label and the detail is one click away.
///
/// A dot and a word, never a filled badge (§6): `--ok` for a copy that is here, `--warn` for one
/// the manifest has moved past. An item nothing has been installed from says nothing at all rather
/// than "not installed" — the absence *is* the answer, and repeating it down a listing of twenty
/// items is twenty words that never change.
fn here_note(app: &MarketplaceApp, elements: &[MarketplaceBundleElement]) -> AnyView {
    let installs = app.bundle.as_ref().map(|b| b.installs.clone()).unwrap_or_default();
    let landed: usize = installs.iter().map(|i| i.elements.len()).sum();
    let outdated = app
        .bundle
        .as_ref()
        .is_some_and(|b| b.installs.iter().any(|i| i.outdated))
        || app.installs.iter().any(|i| i.outdated);
    let state = if !app.installs.is_empty() {
        // The legacy single-dashboard shape counts copies, not elements: two copies of one app is
        // ordinary there, and a fraction would be a fraction of one.
        match app.installs.len() {
            1 => "installed".to_string(),
            n => format!("{n} copies installed"),
        }
    } else if installs.is_empty() {
        String::new()
    } else if installs.len() > 1 {
        // Which projects, not how many elements: the interesting fact about a bundle installed
        // twice is that it is installed twice.
        format!("installed in {} places", installs.len())
    } else if landed == elements.len() {
        format!("installed {}", where_installed(&installs[0]))
    } else {
        format!("{landed} of {} installed {}", elements.len(), where_installed(&installs[0]))
    };

    view! {
        {(!state.is_empty()).then(|| view! {
            <span class="adi-market__here"><span class="adi-dot adi-dot--ok"></span>{state}</span>
        })}
        {outdated.then(|| view! {
            <span class="adi-market__here"><span class="adi-dot adi-dot--warn"></span>"update waiting"</span>
        })}
    }
    .into_any()
}

/// Where one install is filed, as a listing row says it in passing.
fn where_installed(install: &MarketplaceBundleInstall) -> String {
    match (&install.project_name, &install.project) {
        (Some(name), _) => format!("in {name}"),
        (None, Some(id)) => format!("in {id}"),
        (None, None) => "globally".to_string(),
    }
}

/// Every element an item offers, installed here or not — the merged list the store computes when
/// anything is known about the bundle, and the manifest's own preview when nothing is.
///
/// Both shapes answer the same question ("what is in this"), and the listing asks it of every item
/// including the ones nothing has ever been installed from, which is the case `app.bundle` is
/// `None` for.
fn bundle_elements(app: &MarketplaceApp) -> Vec<MarketplaceBundleElement> {
    match &app.bundle {
        Some(bundle) => bundle.elements.clone(),
        None => app
            .elements
            .iter()
            .map(|el| MarketplaceBundleElement {
                kind: preview_kind_dir(&el.kind),
                name: el.name.clone(),
                description: el.description.clone(),
                id: None,
            })
            .collect(),
    }
}

/// The manifest's singular `elements[].kind` as the directory word the rest of this page addresses
/// a kind by. A word this build does not know is passed through untouched, so a manifest several
/// versions ahead still lists what it carries rather than dropping it (the same tolerance the store
/// gives every other unrecognised field).
fn preview_kind_dir(wire: &str) -> String {
    match wire {
        "agent" => "agents",
        "tool" => "tools",
        "dashboard" => "dashboards",
        "embedding" => "embeddings",
        "service" => "services",
        "trigger" => "triggers",
        other => other,
    }
    .to_string()
}

/// What an item carries, as a shelf label says it: one phrase per kind it has an element of, in
/// [`KIND_ORDER`] — "agent · 2 tools · dashboard".
///
/// Not "4 elements": the number is the one thing about a bundle nobody is shopping for, and the
/// kinds are the thing they are. Empty for the legacy single-dashboard shape, which carries exactly
/// one thing and says so in its own name.
fn contents_note(elements: &[MarketplaceBundleElement]) -> String {
    KIND_ORDER
        .iter()
        .filter_map(|kind| {
            let n = elements.iter().filter(|el| el.kind == *kind).count();
            match n {
                0 => None,
                1 => Some(kind_label(kind).to_string()),
                n => Some(format!("{n} {}", kind_plural(kind))),
            }
        })
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

/// A repository as a row says it: the host and path, with the scheme and the `.git` dropped.
///
/// A `file://` repository — a bundle being developed against a local checkout — keeps only its last
/// two segments behind an ellipsis, because the 60 characters of somebody's home directory in front
/// of them say nothing about whose code it is. The row's `title` carries the whole string either
/// way, and the item's own page prints it in full.
fn repo_short(repo: &str) -> String {
    let rest = repo
        .strip_prefix("https://")
        .or_else(|| repo.strip_prefix("http://"))
        .or_else(|| repo.strip_prefix("git://"));
    if let Some(rest) = rest {
        return rest.trim_end_matches(".git").to_string();
    }
    let Some(path) = repo.strip_prefix("file://") else {
        return repo.to_string();
    };
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let mut tail: Vec<&str> = path.rsplit('/').take(2).collect();
    tail.reverse();
    format!("\u{2026}/{}", tail.join("/"))
}

/// What a click on a link into the marketplace should do, given where this listing is drawn.
///
/// Inside the door, a plain left click is taken over: the address bar moves, the open item changes,
/// and the document stays put. Everywhere else — and for a middle click, or a ⌘-click — the
/// browser is left to do what the `href` says, which is why these are links in the first place.
fn open_app(open: OpenApp, key: String) -> impl Fn(&web_sys::MouseEvent) + Clone + 'static {
    move |ev| {
        let Some(open) = open else {
            return;
        };
        if !crate::routing::spa_nav(ev) {
            return;
        }
        let path = match key.split_once('/') {
            Some((marketplace, slug)) => crate::routing::market_app_path(marketplace, slug),
            None => crate::routing::MARKET.to_string(),
        };
        crate::routing::push_state(&path);
        open.set(key.clone());
        crate::routing::scroll_top();
    }
}

/// The way back to the listing, at the head of an item's page. The empty key is the listing, so
/// this is [`open_app`] pointed at nothing in particular.
fn back_link(open: OpenApp) -> AnyView {
    let go = open_app(open, String::new());
    view! {
        <a class="adi-market__back" href=crate::routing::MARKET on:click=move |ev| go(&ev)>
            <Icon icon=Lucide::ArrowLeft size=IconSize::Sm/>
            "All items"
        </a>
    }
    .into_any()
}

/// The item's mark, at the head of its row: the image its manifest publishes, or a tile with the
/// package glyph when it publishes none.
///
/// A tile either way, and the same size either way, so a list where only some publish an icon still
/// reads as one column of rows rather than as a ragged left edge. The image is decorative — the
/// name is right beside it — so its `alt` is empty rather than repeating the name to a screen
/// reader that has just read it.
///
/// **This is the one element on the page that fetches from a host the operator did not choose**:
/// the manifest's publisher did. Which is why the store only accepts `https://` and
/// `data:image/…` (`docs/marketplace.md`), and why a fetch that fails falls back to the same tile
/// — a listing has no business looking damaged because somebody else's host is down, and the
/// browser's broken-image glyph says nothing a reader can act on.
fn app_icon(app: &MarketplaceApp) -> AnyView {
    let Some(src) = app
        .icon
        .as_deref()
        .map(str::trim)
        .filter(|icon| !icon.is_empty())
        .map(str::to_string)
    else {
        return blank_icon();
    };
    let failed = RwSignal::new(false);
    view! {
        {move || {
            if failed.get() {
                return blank_icon();
            }
            view! {
                <img class="adi-market__icon" src=src.clone() alt=""
                    loading="lazy" decoding="async"
                    on:error=move |_| failed.set(true)/>
            }
            .into_any()
        }}
    }
    .into_any()
}

/// The tile an entry gets when it publishes no icon — or when the one it publishes will not load.
fn blank_icon() -> AnyView {
    view! {
        <div class="adi-market__icon adi-market__icon--blank" aria-hidden="true">
            <Icon icon=Lucide::Package/>
        </div>
    }
    .into_any()
}

/// What the entry says it is about, as tags (§6 "Tag": sans, 12px, pill).
///
/// The publisher's own words and their own order — the store has already trimmed them and dropped
/// the repeats. On the item's own page rather than on the row: a shelf label already says what the
/// thing is twice (its name and its line), and a third row of words is what made the first version
/// of this listing unreadable.
fn keyword_tags(app: &MarketplaceApp) -> Option<AnyView> {
    if app.keywords.is_empty() {
        return None;
    }
    Some(
        view! {
            <div class="adi-market__tags">
                {app.keywords.iter()
                    .map(|word| view! { <span class="adi-chip">{word.clone()}</span> })
                    .collect::<Vec<_>>()}
            </div>
        }
        .into_any(),
    )
}

/// One item's own page — what it is, what it looks like, what is in it, and what would be cloned.
///
/// The order is the order somebody reads it in, and it is the order a store page has had since
/// stores had pages: the head (mark, name, one line, the act), then the pictures, then the promise
/// this machine can actually keep, then the contents with an action each, then the long form, and
/// only then the machine facts. The facts come last and never first — a person deciding whether to
/// install something is not reading a commit id, and a person checking a commit id knows where to
/// scroll.
fn app_page(
    state: State,
    form: MarketplaceForm,
    app: &MarketplaceApp,
    open: RwSignal<String>,
) -> AnyView {
    let owned = app.clone();
    view! {
        <div class="adi-market__app">
            {back_link(Some(open))}
            // The head is one block rather than three siblings: the page is a flex column with a
            // 32px gap, and a form that is closed — or a flash with nothing to say — would
            // otherwise still take a gap each and leave a hole under the title.
            <div class="adi-market__head">
                {hero(form, app)}
                {flash_view(state.flash)}
            </div>
            {gallery_view(app)}
            {assurances()}
            {included_view(state, form, app)}
            {installs_view(state, form, app)}
            {installed_here(state, form, app)}
            {readme_view(app)}
            {facts_view(&owned)}
            {install_dialog(state, form, app)}
        </div>
    }
    .into_any()
}

/// The head of an item's page: the mark, what this is in one line above the name, the name, the
/// publisher's own sentence, its tags — and the one act the page is for.
fn hero(form: MarketplaceForm, app: &MarketplaceApp) -> AnyView {
    let (name, version, description) = (
        app.name.clone(),
        app.version.clone(),
        app.description.clone(),
    );
    view! {
        <header class="adi-market__hero">
            <div class="adi-market__appmark">{app_icon(app)}</div>
            <div class="adi-market__appabout">
                <div class="adi-market__eyebrow">{eyebrow(app)}</div>
                <h1 class="adi-market__apptitle">
                    {name}
                    {version.map(|v| view! { <span class="adi-mono adi-muted">{v}</span> })}
                </h1>
                {description.map(|d| view! { <p class="adi-market__applead">{d}</p> })}
                {keyword_tags(app)}
            </div>
            <div class="adi-market__heroact">{hero_action(form, app)}</div>
        </header>
    }
    .into_any()
}

/// The line above the name: which of the two shapes this is, how much is in it, and which
/// marketplace it came from — the three facts that decide whether the page below is worth reading.
fn eyebrow(app: &MarketplaceApp) -> String {
    let elements = bundle_elements(app);
    let what = match elements.len() {
        0 => "App".to_string(),
        1 => "Bundle \u{b7} one element".to_string(),
        n => format!("Bundle \u{b7} {n} elements"),
    };
    format!("{what} \u{b7} from {}", app.marketplace)
}

/// The page's one act, in whichever of its four states this item is in — and the screen's one
/// orange (§8), which is why nothing else on this page is filled.
///
/// * The **legacy single-dashboard shape** opens the name form, and the form's own Install is the
///   orange. This button is not, because pressing it only asks a question.
/// * A **general bundle** installs every element it offers, with no form at all: `name` and
///   `start` mean nothing to it (`docs/marketplace-bundles.md` — every element lands under its own
///   published name), so asking would be asking about something that will be ignored.
/// * A **partly installed** bundle offers the rest. An element already in the ledger is left as it
///   stands by the installer itself, so this is safe to press at any time.
/// * A **fully installed** one offers nothing here: what is left to do to it is per element, in
///   the list below, and Update when the manifest has moved.
fn hero_action(form: MarketplaceForm, app: &MarketplaceApp) -> AnyView {
    let busy = form.busy;
    let elements = bundle_elements(app);
    let installs = app.bundle.as_ref().map(|b| b.installs.len()).unwrap_or(0)
        + app.installs.len();
    let legacy = is_legacy(app);
    let owned = app.clone();

    // Every destination question is the dialog's; this button's only job is to ask it. Which is
    // also why it says "Install" and not "Install everything" once a copy is already here: the
    // second one is a *copy*, and the dialog is where that becomes clear.
    let label = match (installs, elements.len()) {
        (0, 0 | 1) => "Install".to_string(),
        (0, _) => "Install everything".to_string(),
        _ if legacy => "Install another copy".to_string(),
        _ => "Install another copy".to_string(),
    };
    let note = if legacy {
        "you name your copy"
    } else if installs == 0 {
        "nothing runs until you start it"
    } else {
        "a second copy, in a project of its own"
    };

    view! {
        <button class="adi-btn adi-btn--accent" type="button"
            prop:disabled=move || busy.get().is_some()
            on:click=move |_| open_dialog(form, &owned, "")>
            <Icon icon=Lucide::Download size=IconSize::Sm/>
            {label}
        </button>
        <span class="adi-market__heronote">{note}</span>
    }
    .into_any()
}

/// The v1 shape: no elements, no bundle status — one dashboard and nothing else
/// (`docs/marketplace-bundles.md` decision #6).
fn is_legacy(app: &MarketplaceApp) -> bool {
    app.bundle.is_none() && app.elements.is_empty()
}

/// Open the install dialog for this item — the whole bundle, or the one `<kind>/<name>` named.
///
/// Seeded every time it opens rather than once: the destination most installs want is a project
/// that does not exist yet, named after the bundle, and an operator who installed something else
/// five minutes ago should not find that install's answers still in the fields.
fn open_dialog(form: MarketplaceForm, app: &MarketplaceApp, element: &str) {
    form.element.set(element.to_string());
    form.name.set(app.name.clone());
    form.start_now.set(true);
    form.new_project.set(suggested_project(app));
    form.dest.set(Destination::New);
    form.installing.set(app_key(app));
}

/// Open the install dialog for `key` (`<marketplace>/<slug>`), if this listing carries that item.
///
/// What a `?install=1` link out of the ADI Store asks for — somebody pressed Install out there, so
/// the dialog is what they are waiting for. It goes through [`open_dialog`] rather than setting
/// `form.installing` directly, because the seeding is half of what pressing that button means: a
/// dialog opened without it offers to register a project called "".
///
/// Called once the listing has arrived, since until then there is no item to seed from.
pub(crate) fn open_install(form: MarketplaceForm, loaded: &MarketplaceState, key: &str) {
    if let Some(app) = loaded.apps.iter().find(|app| app_key(app) == key) {
        open_dialog(form, app, "");
    }
}

/// The name to offer for the project an install would register: the one the bundle's own scaffold
/// publishes when it ships one — that is the publisher's own answer to "what is this for" — and
/// the item's name otherwise.
fn suggested_project(app: &MarketplaceApp) -> String {
    bundle_elements(app)
        .iter()
        .find(|el| el.kind == "project")
        .map(|el| el.name.clone())
        .unwrap_or_else(|| app.name.clone())
}

/// The three things this machine can actually promise about installing somebody else's item, in
/// one line under the pictures.
///
/// Not a marketing strip: every one of the three is a property the store implements and
/// `docs/marketplace.md` argues for — the clone is local, arrival is inert, and the pin is the
/// whole identity of what lands. They sit here, between the pictures and the contents, because
/// this is the moment in the page where somebody has decided they want it and started wondering
/// what it costs them.
fn assurances() -> AnyView {
    view! {
        <ul class="adi-market__assurances">
            <li>
                <Icon icon=Lucide::Laptop size=IconSize::Sm/>
                "Cloned to this machine \u{2014} no account, nothing phoned home"
            </li>
            <li>
                <Icon icon=Lucide::Power size=IconSize::Sm/>
                "Installed is not started: you decide what runs"
            </li>
            <li>
                <Icon icon=Lucide::GitCommitHorizontal size=IconSize::Sm/>
                "Pinned to one commit \u{2014} what you read is what installs"
            </li>
        </ul>
    }
    .into_any()
}

/// The eight kind directories, in the fixed order the Rust side already sorts by
/// (`adi_marketplace::Kind::ALL`) — repeated here rather than derived, since the wire type carries
/// the spelling but not the enum. One order for every bundle is the whole point: the eye learns it
/// once, and never has to re-learn it item to item.
const KIND_ORDER: [&str; 8] =
    ["agents", "tools", "dashboards", "llm", "embeddings", "services", "triggers", "project"];

/// What one element of this kind is called, in the singular — the word on its row.
fn kind_label(kind: &str) -> &str {
    match kind {
        "agents" => "agent",
        "tools" => "tool",
        "dashboards" => "dashboard",
        "llm" => "LLM backend",
        "embeddings" => "embedding backend",
        "services" => "hive service",
        "triggers" => "trigger",
        "project" => "project",
        // A kind this build does not know: the manifest is ahead of this binary. Drawn as
        // published rather than hidden, the same tolerance the rest of the page gives it.
        other => other,
    }
}

/// The same word for more than one of them, for a shelf label's "2 tools".
fn kind_plural(kind: &str) -> String {
    match kind {
        "llm" => "LLM backends".to_string(),
        "embeddings" => "embedding backends".to_string(),
        "services" => "hive services".to_string(),
        other => format!("{}s", kind_label(other)),
    }
}

/// The glyph for a kind, taken from the panel's own noun→icon map (`crate::icons`, DESIGN.md §9)
/// rather than picked again here: an agent is a bot on the Agents page and on this one, or the map
/// is not a map.
fn kind_icon(kind: &str) -> Lucide {
    match kind {
        "agents" => Lucide::Bot,
        "tools" => Lucide::Wrench,
        "dashboards" => Lucide::LayoutDashboard,
        "llm" => Lucide::Brain,
        "embeddings" => Lucide::ScanLine,
        "services" => Lucide::Server,
        "triggers" => Lucide::Zap,
        "project" => Lucide::Folder,
        _ => Lucide::Package,
    }
}

/// **What's included**: everything the item offers, one row each, with the one act that applies.
///
/// The catalogue — about the *bundle*, not about any one install of it. So no ids here and no
/// "installed" state: a bundle may be installed in three projects, and a row that tried to say
/// what it landed as would have to pick one. Each row's Install opens the same dialog the head's
/// does, aimed at that one element.
///
/// Flat and in [`KIND_ORDER`], with the kind on the row itself — not eight headings over eight
/// single rows, which is what the first version drew and which made a bundle of one of everything
/// read as an outline rather than as a list of things you can have.
fn included_view(state: State, form: MarketplaceForm, app: &MarketplaceApp) -> Option<AnyView> {
    let elements = bundle_elements(app);
    if elements.is_empty() {
        return None;
    }
    let owned = app.clone();
    Some(
        view! {
            <section class="adi-market__included">
                <div class="adi-market__sechead">
                    <h2 class="adi-market__sectitle">"What's included"</h2>
                    <span class="adi-market__state">
                        {match elements.len() {
                            1 => "one element — take it on its own, or with the bundle".to_string(),
                            n => format!("{n} elements — install the lot, or one at a time"),
                        }}
                    </span>
                </div>
                <ul class="adi-market__elements">
                    {elements.iter()
                        .map(|el| catalogue_row(state, form, &owned, el))
                        .collect::<Vec<_>>()}
                </ul>
            </section>
        }
        .into_any(),
    )
}

/// One element of the catalogue: what it is, what it does, and Install — which asks where.
fn catalogue_row(
    state: State,
    form: MarketplaceForm,
    app: &MarketplaceApp,
    el: &MarketplaceBundleElement,
) -> AnyView {
    let _ = state;
    let busy = form.busy;
    let spec = element_spec(&el.kind, &el.name);
    let owned = app.clone();
    view! {
        <li class="adi-market__element">
            {element_about(el)}
            <span class="adi-spacer"></span>
            <button class="adi-btn" type="button"
                prop:disabled=move || busy.get().is_some()
                on:click=move |_| open_dialog(form, &owned, &spec)>
                "Install"
            </button>
        </li>
    }
    .into_any()
}

/// The `<kind>/<name>` address this element installs, uninstalls or starts under — `project` alone
/// for the one singleton kind, which an address never gives a name of its own.
fn element_spec(kind: &str, name: &str) -> String {
    if kind == "project" {
        "project".to_string()
    } else {
        format!("{kind}/{name}")
    }
}

/// The left of an element row, wherever it is drawn: the kind's glyph, the published name, the
/// kind as a tag, and the publisher's line.
fn element_about(el: &MarketplaceBundleElement) -> AnyView {
    let (kind, name) = (el.kind.clone(), el.name.clone());
    let description = el.description.clone();
    let landed = el.id.clone().filter(|id| *id != name);
    view! {
        <span class="adi-market__elementicon" aria-hidden="true">
            <Icon icon=kind_icon(&kind)/>
        </span>
        <span class="adi-market__elementabout">
            <span class="adi-market__elementname">
                {name}
                <span class="adi-market__kind">{kind_label(&kind).to_string()}</span>
                // Only when they differ: a copy that had to mint an id of its own is the one case
                // where the name the publisher chose is not what an operator will type afterwards.
                {landed.map(|id| view! {
                    <span class="adi-mono adi-muted">"\u{2192} "{id}</span>
                })}
            </span>
            {description.map(|d| view! { <span class="adi-market__elementdesc">{d}</span> })}
        </span>
    }
    .into_any()
}

/// **Installed here**: one block per install, because two installs of one bundle are two copies —
/// in different projects, at possibly different commits, updated and uninstalled independently.
///
/// Where the catalogue above says what the bundle offers, this says what this machine actually has
/// and under which project. It is the section that makes "the changelog tools in project A and
/// again in project B" a thing you can see rather than a thing you have to remember.
fn installs_view(state: State, form: MarketplaceForm, app: &MarketplaceApp) -> Option<AnyView> {
    let bundle = app.bundle.as_ref()?;
    if bundle.installs.is_empty() {
        return None;
    }
    let total = bundle.elements.len();
    let (marketplace, slug) = (app.marketplace.clone(), app.slug.clone());
    Some(
        view! {
            <section class="adi-market__installs">
                <div class="adi-market__sechead">
                    <h2 class="adi-market__sectitle">"Installed here"</h2>
                    <span class="adi-market__state">
                        {match bundle.installs.len() {
                            1 => String::new(),
                            n => format!("{n} copies, each with its own pin"),
                        }}
                    </span>
                </div>
                {bundle.installs.iter()
                    .map(|install| install_block(state, form, &marketplace, &slug, install, total))
                    .collect::<Vec<_>>()}
            </section>
        }
        .into_any(),
    )
}

/// One install: where it is filed, where it stands, what it landed, and what is left to do to it.
fn install_block(
    state: State,
    form: MarketplaceForm,
    marketplace: &str,
    slug: &str,
    install: &MarketplaceBundleInstall,
    total: usize,
) -> AnyView {
    let busy = form.busy;
    let project = install.project.clone();
    let where_ = match (&install.project, &install.project_name) {
        (Some(_), Some(name)) => format!("in {name}"),
        (Some(id), None) => format!("in {id}"),
        (None, _) => "globally".to_string(),
    };
    let commit = short_commit(&install.commit);
    let (missing, outdated) = (install.missing_secrets.clone(), install.outdated);
    let landed = install.elements.len();
    let (m, s) = (marketplace.to_string(), slug.to_string());
    let update_key = format!("bundle-update:{m}/{s}/{}", project.clone().unwrap_or_default());
    let (um, us, up) = (m.clone(), s.clone(), project.clone());

    view! {
        <div class="adi-market__install">
            <div class="adi-market__installhead">
                <span class="adi-market__installwhere">
                    <Icon icon=if project.is_some() { Lucide::Folder } else { Lucide::Globe }
                        size=IconSize::Sm/>
                    {where_}
                </span>
                <span class="adi-market__state">
                    {format!("{landed} of {total} elements \u{b7} ")}
                    <span class="adi-mono">{commit}</span>
                </span>
                <span class="adi-spacer"></span>
                {outdated.then(move || view! {
                    <span class="adi-market__here">
                        <span class="adi-dot adi-dot--warn"></span>"update waiting"
                    </span>
                    <button class="adi-btn" type="button"
                        prop:disabled=move || busy.get().is_some()
                        on:click=move |_| {
                            run(state, form, update_key.clone(),
                                fetch::update_marketplace_bundle(
                                    um.clone(), us.clone(), Vec::new(), up.clone()));
                        }>
                        <Icon icon=Lucide::ArrowUp size=IconSize::Sm/>
                        "Update"
                    </button>
                })}
            </div>
            <ul class="adi-market__elements">
                {install.elements.iter()
                    .map(|el| installed_row(state, form, &m, &s, project.clone(), el))
                    .collect::<Vec<_>>()}
            </ul>
            {(!missing.is_empty()).then(|| view! {
                <p class="adi-hint">
                    {format!(
                        "Set {} \u{2014} the elements that name it will not work until you do.",
                        missing.join(", ")
                    )}
                </p>
            })}
        </div>
    }
    .into_any()
}

/// One element inside an install: what it landed as, and — in the row's `\u{22ef}` — the acts that
/// only make sense for something already here (§8: row actions in a menu, never a destructive word
/// repeated down a column). A hive service, which arrives parked, gets Start there too; Start is
/// idempotent on one already started, so offering it always costs nothing.
fn installed_row(
    state: State,
    form: MarketplaceForm,
    marketplace: &str,
    slug: &str,
    project: Option<String>,
    el: &MarketplaceBundleElement,
) -> AnyView {
    let spec = element_spec(&el.kind, &el.name);
    let is_service = el.kind == "services";
    let (m, s) = (marketplace.to_string(), slug.to_string());
    let scope_key = project.clone().unwrap_or_default();
    let start_key = format!("start-service:{m}/{s}/{spec}/{scope_key}");
    let uninstall_key = format!("uninstall:{m}/{s}/{spec}/{scope_key}");
    let (sm, ss, sname, sp) = (m.clone(), s.clone(), el.name.clone(), project.clone());
    let (um, us, uspec, up) = (m.clone(), s.clone(), spec.clone(), project.clone());
    let confirm_spec = spec.clone();
    let where_ = match &project {
        Some(id) => format!(" from {id}"),
        None => String::new(),
    };

    let mut items = Vec::new();
    if is_service {
        items.push(menu_item(state, "Start", false, move || {
            run(state, form, start_key.clone(),
                fetch::start_marketplace_service(sm.clone(), ss.clone(), sname.clone(), sp.clone()));
        }));
    }
    items.push(menu_item(state, "Uninstall", true, move || {
        if confirm(&format!(
            "Uninstall {confirm_spec}{where_}? Its siblings in this install are untouched, and so \
             is any other copy of this bundle."
        )) {
            run(state, form, uninstall_key.clone(),
                fetch::uninstall_marketplace_element(um.clone(), us.clone(), uspec.clone(), up.clone()));
        }
    }));

    view! {
        <li class="adi-market__element is-installed">
            {element_about(el)}
            <span class="adi-spacer"></span>
            {row_actions(state, format!("market:{m}/{s}/{spec}/{scope_key}"), (), items)}
        </li>
    }
    .into_any()
}

/// The copies of a legacy single-dashboard item that are already here, with what each allows.
/// A general bundle says the same thing per element, in [`included_view`], so this is drawn only
/// for the shape that has copies at all.
fn installed_here(state: State, form: MarketplaceForm, app: &MarketplaceApp) -> Option<AnyView> {
    if app.installs.is_empty() {
        return None;
    }
    let installs = app.installs.clone();
    Some(
        view! {
            <section class="adi-market__copies">
                <h2 class="adi-market__sectitle">"Installed here"</h2>
                {installs.iter()
                    .map(|install| copy_row(state, form, install))
                    .collect::<Vec<_>>()}
            </section>
        }
        .into_any(),
    )
}

/// The gallery: one picture or clip at a time, with the rest as thumbnails under it.
///
/// A stage rather than a grid of squares, because these are screenshots of a working thing and a
/// screenshot shrunk to a tile says nothing. Under it, the publisher's caption on the left and —
/// only when there is more than one — where you are in the set and the two arrows that move you,
/// on the right.
///
/// Nothing autoplays: a clip is a `<video controls>` that waits to be asked (§8 — no motion the
/// reader did not trigger), and `preload=metadata` keeps a page of clips from pulling megabytes
/// nobody asked for.
fn gallery_view(app: &MarketplaceApp) -> Option<AnyView> {
    if app.gallery.is_empty() {
        return None;
    }
    let items = app.gallery.clone();
    let count = items.len();
    // Which one is on the stage. Per item page, and the page is rebuilt when the item changes, so
    // opening a second item never opens it at somebody else's third screenshot.
    let at = RwSignal::new(0usize);
    let (staged, captioned, thumbs) = (items.clone(), items.clone(), items.clone());
    Some(
        view! {
            <section class="adi-market__gallery">
                {move || {
                    let shown = at.get().min(count.saturating_sub(1));
                    staged.get(shown).map(stage_item)
                }}
                <div class="adi-market__galleryfoot">
                    <span class="adi-market__caption">
                        {move || captioned
                            .get(at.get().min(count.saturating_sub(1)))
                            .and_then(|item| item.caption.clone())
                            .filter(|c| !c.trim().is_empty())
                            .unwrap_or_default()}
                    </span>
                    <span class="adi-spacer"></span>
                    {(count > 1).then(move || view! {
                        <span class="adi-market__counter">
                            {move || format!("{} of {count}", at.get() + 1)}
                        </span>
                        <button class="adi-btn adi-btn--icon-sm" type="button" aria-label="Previous"
                            on:click=move |_| at.update(|i| *i = (*i + count - 1) % count)>
                            <Icon icon=Lucide::ChevronLeft size=IconSize::Sm/>
                        </button>
                        <button class="adi-btn adi-btn--icon-sm" type="button" aria-label="Next"
                            on:click=move |_| at.update(|i| *i = (*i + 1) % count)>
                            <Icon icon=Lucide::ChevronRight size=IconSize::Sm/>
                        </button>
                    })}
                </div>
                {(thumbs.len() > 1).then(|| view! {
                    <div class="adi-market__thumbs">
                        {thumbs.iter().enumerate().map(|(i, item)| {
                            thumb(item, i, at)
                        }).collect::<Vec<_>>()}
                    </div>
                })}
            </section>
        }
        .into_any(),
    )
}

/// What is on the stage: the picture, or the player.
fn stage_item(item: &MarketplaceMedia) -> AnyView {
    let (url, poster) = (item.url.clone(), item.poster.clone());
    view! {
        <div class="adi-market__stage">
            {match item.kind {
                MediaKind::Video => view! {
                    <video class="adi-market__media" controls preload="metadata"
                        poster=poster.unwrap_or_default() src=url/>
                }
                .into_any(),
                MediaKind::Image => view! {
                    <img class="adi-market__media" src=url alt="" loading="lazy" decoding="async"/>
                }
                .into_any(),
            }}
        </div>
    }
    .into_any()
}

/// One thumbnail: the picture itself, or a clip's poster with a play glyph over it — and for a
/// clip with no poster, the glyph alone, which is still the honest answer.
fn thumb(item: &MarketplaceMedia, i: usize, at: RwSignal<usize>) -> AnyView {
    let video = item.kind == MediaKind::Video;
    let still = if video {
        item.poster.clone().filter(|p| !p.trim().is_empty())
    } else {
        Some(item.url.clone())
    };
    let label = if video {
        format!("Clip {}", i + 1)
    } else {
        format!("Picture {}", i + 1)
    };
    view! {
        <button class="adi-market__thumb" type="button"
            class:is-on=move || at.get() == i
            aria-label=label
            on:click=move |_| at.set(i)>
            {still.map(|src| view! {
                <img class="adi-market__thumbimg" src=src alt="" loading="lazy" decoding="async"/>
            })}
            {video.then(|| view! {
                <span class="adi-market__play"><Icon icon=Lucide::Play size=IconSize::Sm/></span>
            })}
        </button>
    }
    .into_any()
}

/// The long form, rendered.
///
/// [`adi_ui::Markdown`] renders through Leptos views rather than `innerHTML`, so a manifest from
/// anywhere cannot inject markup into this page however it is written — which is the whole reason
/// a publisher's prose can be shown at all.
fn readme_view(app: &MarketplaceApp) -> Option<AnyView> {
    let readme = app
        .readme
        .clone()
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())?;
    Some(
        view! {
            // No heading of our own over it: the publisher's prose brings its own, and two
            // headings for one section is what "About this item / What it is" reads as.
            <section class="adi-market__readme">
                <Markdown source=readme/>
            </section>
        }
        .into_any(),
    )
}

/// What would actually be cloned, at the foot of the page: the repository, the commit, the branch
/// and the address — in mono, because every one of them is a string a machine consumes.
///
/// The listing text above is the publisher's; this is the part that is checkable, and it is last
/// because checking it is what somebody does after they have decided to care.
fn facts_view(app: &MarketplaceApp) -> AnyView {
    let (repo, commit) = (app.repo.clone(), short_commit(&app.commit));
    let (repo_title, commit_title) = (repo.clone(), app.commit.clone());
    let branch = app.branch.clone().filter(|b| !b.trim().is_empty());
    let key = app_key(app);
    view! {
        <section class="adi-market__facts">
            <span class="adi-market__fact">
                <span class="adi-market__factkey">"Repository"</span>
                <span class="adi-mono adi-muted" title=repo_title>{repo}</span>
            </span>
            <span class="adi-market__fact">
                <span class="adi-market__factkey">"Commit"</span>
                <span class="adi-mono adi-muted" title=commit_title>{commit}</span>
            </span>
            {branch.map(|b| view! {
                <span class="adi-market__fact">
                    <span class="adi-market__factkey">"Branch"</span>
                    <span class="adi-mono adi-muted">{b}</span>
                </span>
            })}
            <span class="adi-market__fact">
                <span class="adi-market__factkey">"Address"</span>
                <span class="adi-mono adi-muted">{key}</span>
            </span>
        </section>
    }
    .into_any()
}

/// The install dialog: **where does this go?**
///
/// The question the panel used to not ask at all — everything landed globally, and the same bundle
/// could only ever be installed once. Three destinations, and they are deliberately not equal:
///
/// * **A new project** is the default and the recommendation. It is the only destination that lets
///   the same bundle be installed twice (every kind's ids live in one global namespace, so a second
///   global copy of a tool has nowhere to go), and it files the bundle's tools, agents and triggers
///   under a project of their own, where they run in that project's directory and against its
///   database.
/// * **A project you already have** is the same thing, for the second bundle that belongs with the
///   first.
/// * **Globally** is offered, marked, and never the default: every agent on the machine can reach
///   what lands there, it spends the shared id namespace, and a second copy of this bundle will be
///   refused rather than renamed.
///
/// The legacy single-dashboard shape asks its own two questions here too (what to call the copy,
/// and whether to start it), because for that shape they are the whole of the install.
fn install_dialog(state: State, form: MarketplaceForm, app: &MarketplaceApp) -> AnyView {
    let key = app_key(app);
    // `Modal`'s children may be built more than once, so anything the submit handler owns has to
    // survive being captured again — a `StoredValue` is `Copy` and does, where a `String` moved
    // into the closure makes that closure `FnOnce` and the children closure with it.
    let addr = StoredValue::new((app.marketplace.clone(), app.slug.clone()));
    let legacy = is_legacy(app);
    let open = RwSignal::new(false);
    // The dialog is one component per item page; `form.installing` is the signal that opens it, so
    // the two are kept in step rather than duplicated.
    let key = StoredValue::new(key);
    // Two signals, kept in step in both directions — and **each write is guarded by a read of the
    // other side untracked**. Without the guard these two effects feed each other: `set` notifies
    // whether or not the value changed, so opening the dialog once spins the reactive graph
    // forever, which shows up as a page that never settles and a browser that gets OOM-killed
    // rather than as anything that looks like a loop.
    Effect::new(move |_| {
        let wanted = form.installing.get() == key.get_value();
        if open.get_untracked() != wanted {
            open.set(wanted);
        }
    });
    // The other direction: a dialog dismissed by the scrim, the × or Escape has to put the signal
    // that opened it back, or the button that opens it stops working.
    Effect::new(move |_| {
        if !open.get() && form.installing.get_untracked() == key.get_value() {
            form.installing.set(String::new());
        }
    });

    let title = format!("Install {}", app.name);

    view! {
        <Modal open=open title=title width="max-w-lg">
            <form class="adi-form adi-market__dialog" on:submit=move |ev| {
                ev.prevent_default();
                let element = form.element.get();
                let (project, new_project) = match form.dest.get() {
                    Destination::New => (None, Some(form.new_project.get().trim().to_string())),
                    Destination::Existing => (Some(form.project.get()), None),
                    Destination::Global => (None, None),
                };
                open.set(false);
                let (marketplace, slug) = addr.get_value();
                run(
                    state,
                    form,
                    format!("install:{marketplace}/{slug}"),
                    fetch::install_marketplace_app(
                        marketplace,
                        slug,
                        (!element.is_empty()).then_some(element),
                        form.name.get().trim().to_string(),
                        form.start_now.get(),
                        project.filter(|p| !p.is_empty()),
                        new_project.filter(|p| !p.is_empty()),
                    ),
                );
            }>
                <p class="adi-market__dialoglead">
                    // Which element, when it is one element — the dialog's title names the item,
                    // and this is the only other thing it could be about.
                    {move || match form.element.get() {
                        el if el.is_empty() => String::new(),
                        el => format!("Installing {el} on its own. "),
                    }}
                    "Where should this be filed? A project of its own is what we recommend \u{2014} \
                     it keeps this bundle's tools and agents in one place, and it is the only way \
                     to install the same bundle more than once."
                </p>

                {destination_option(
                    form, Destination::New, "A new project",
                    "Registered now, named below. Recommended.", None,
                )}
                <div class="adi-market__dialogfield" class:is-off=move || form.dest.get() != Destination::New>
                    <TextField id="market-new-project" label="Call the project"
                        placeholder="On-call kit" value=form.new_project />
                </div>

                {destination_option(
                    form, Destination::Existing, "A project you already have",
                    "Files everything under it, beside whatever is there.", None,
                )}
                <div class="adi-market__dialogfield" class:is-off=move || form.dest.get() != Destination::Existing>
                    {project_select(state, form)}
                </div>

                {destination_option(
                    form, Destination::Global, "Globally",
                    "Every agent on this machine can reach it, and a second copy of this bundle \
                     will be refused rather than renamed.",
                    Some("not recommended"),
                )}

                {legacy.then(|| view! {
                    <div class="adi-market__dialoglegacy">
                        <TextField id="market-name" label="Name it" placeholder="Sales CRM"
                            value=form.name />
                        <label class="adi-field adi-field--check">
                            <input type="checkbox"
                                prop:checked=move || form.start_now.get()
                                on:change=move |ev| form.start_now.set(event_target_checked(&ev)) />
                            <span class="adi-field__label">"Start it right away"</span>
                            {field_hint("this runs the app's own code on this machine")}
                        </label>
                    </div>
                })}

                <div class="adi-market__dialogfoot">
                    <button class="adi-btn adi-btn--ghost" type="button"
                        on:click=move |_| open.set(false)>
                        "Cancel"
                    </button>
                    // Ink, not orange: the page's own Install button behind this dialog is the
                    // screen's one accent (§8), and this is the same act confirmed.
                    <button class="adi-btn adi-btn--primary" type="submit"
                        prop:disabled=move || form.busy.get().is_some()>
                        "Install"
                    </button>
                </div>
            </form>
        </Modal>
    }
    .into_any()
}

/// One destination, as a row you choose: a radio, what it is, and what it costs.
///
/// A radio rather than a segmented control because each choice needs a sentence under it, and a
/// segmented control that wraps prose is neither. The marked one carries a `--warn` dot and the
/// words, never a red row: nothing here is destructive, it is simply the choice that closes doors
/// (§3 — warn is a dot and a word, never coloured prose).
fn destination_option(
    form: MarketplaceForm,
    dest: Destination,
    label: &str,
    why: &str,
    caution: Option<&str>,
) -> AnyView {
    let (label, why) = (label.to_string(), why.to_string());
    let caution = caution.map(str::to_string);
    view! {
        <label class="adi-market__dest" class:is-on=move || form.dest.get() == dest>
            <input type="radio" name="market-destination"
                prop:checked=move || form.dest.get() == dest
                on:change=move |_| form.dest.set(dest) />
            <span class="adi-market__destabout">
                <span class="adi-market__destname">
                    {label}
                    {caution.map(|word| view! {
                        <span class="adi-market__here">
                            <span class="adi-dot adi-dot--warn"></span>{word}
                        </span>
                    })}
                </span>
                <span class="adi-market__destwhy">{why}</span>
            </span>
        </label>
    }
    .into_any()
}

/// The projects this machine has, for the "a project you already have" destination.
///
/// Archived ones are left out: filing a fresh install under a project somebody has put away is a
/// state nobody asked for. A machine with no projects at all says so rather than offering an empty
/// select — the other two destinations still work.
fn project_select(state: State, form: MarketplaceForm) -> AnyView {
    view! {
        {move || {
            let projects: Vec<adi_webapp_api::types::Project> = state
                .projects
                .get()
                .map(|p| p.projects.into_iter().filter(|p| p.archived_at.is_none()).collect())
                .unwrap_or_default();
            if projects.is_empty() {
                return view! {
                    <p class="adi-hint">"No projects yet \u{2014} a new one is the way in."</p>
                }
                .into_any();
            }
            // The first project is the standing answer until somebody picks another, so the field
            // is never submitted empty.
            if form.project.get_untracked().is_empty()
                && let Some(first) = projects.first()
            {
                form.project.set(first.id.clone());
            }
            view! {
                <label class="adi-field">
                    <span class="adi-field__label">"Which project"</span>
                    <select class="adi-input"
                        on:change=move |ev| form.project.set(event_target_value(&ev))>
                        {projects.into_iter().map(|p| {
                            let selected = form.project.get_untracked() == p.id;
                            view! {
                                <option value=p.id.clone() selected=selected>
                                    {p.name.clone()}
                                </option>
                            }
                        }).collect::<Vec<_>>()}
                    </select>
                </label>
            }
            .into_any()
        }}
    }
    .into_any()
}

/// One installed copy: what it is called, where it stands, and the one or two acts it allows.
///
/// A copy that is behind the manifest's pin says so and offers Update — a fast-forward, so an
/// operator's own commits on top of it are never walked over. Force sits in the row's `⋯` beside
/// it, because the refusal is otherwise a dead end inside the panel and because a destructive act
/// does not belong in the same row of buttons as a safe one; it is still gated by a confirm that
/// says what it costs.
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

    let (start_key, update_key, force_key) = (key.clone(), key.clone(), key.clone());
    let (start_id, update_id, force_id) = (id.clone(), id.clone(), id.clone());
    let force_item = outdated.then(|| {
        menu_item(state, "Force update", true, move || {
            if confirm(
                "Reset this copy onto the marketplace's commit? Any changes you made to it here \
                 are lost.",
            ) {
                run(state, form, force_key.clone(),
                    fetch::update_marketplace_app(force_id.clone(), true));
            }
        })
    });

    let inline = view! {
        {outdated.then(move || view! {
            <button class="adi-btn" type="button"
                prop:disabled=move || busy.get().is_some()
                on:click=move |_| {
                    run(state, form, update_key.clone(),
                        fetch::update_marketplace_app(update_id.clone(), false));
                }>
                "Update"
            </button>
        })}
        {if started {
            open_link(host.as_deref())
        } else {
            view! {
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
    };

    view! {
        <div class="adi-market__copy">
            <span class="adi-market__copy-name">{name}</span>
            <span class="adi-mono adi-muted">{id.clone()}" @ "{commit}</span>
            {(!started).then(|| view! { <span class="adi-market__state">"not running"</span> })}
            {outdated.then(|| view! {
                <span class="adi-market__here">
                    <span class="adi-dot adi-dot--warn"></span>"update waiting"
                </span>
            })}
            <span class="adi-spacer"></span>
            {row_actions(state, format!("market-copy:{id}"), inline, force_item.into_iter().collect())}
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
            <span class="adi-market__here">
                <span class="adi-dot adi-dot--ok"></span>{format!("running at {host}")}
            </span>
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

    fn element(kind: &str, name: &str, id: Option<&str>) -> MarketplaceBundleElement {
        MarketplaceBundleElement {
            kind: kind.to_string(),
            name: name.to_string(),
            description: None,
            id: id.map(str::to_string),
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

    #[test]
    fn a_shelf_label_names_the_kinds_in_one_fixed_order_and_counts_only_when_it_has_to() {
        let elements = vec![
            element("tools", "csv-import", None),
            element("agents", "sales-bot", None),
            element("tools", "vcf-import", None),
            element("llm", "gpt5", None),
        ];
        // KIND_ORDER, not the order the elements arrived in; a repeated kind counts, a single one
        // is named.
        assert_eq!(contents_note(&elements), "agent · 2 tools · LLM backend");
        assert_eq!(contents_note(&[]), "", "the legacy shape has nothing to list");
    }

    #[test]
    fn every_kind_has_a_word_a_person_would_say() {
        for kind in KIND_ORDER {
            assert!(!kind_label(kind).is_empty(), "{kind}");
            assert!(kind_plural(kind).ends_with('s'), "{kind}");
            // Never the raw directory word: that is the address spelling, not the reader's.
            if kind != "project" {
                assert_ne!(kind_label(kind), kind, "{kind}");
            }
        }
        // A kind from a newer manifest is drawn as published rather than dropped.
        assert_eq!(kind_label("hologram"), "hologram");
    }

    #[test]
    fn a_repository_is_shortened_without_losing_whose_code_it_is() {
        assert_eq!(
            repo_short("https://github.com/adi-family/crm-suite.git"),
            "github.com/adi-family/crm-suite"
        );
        // A local checkout keeps the two segments that name it, not the home directory in front.
        assert_eq!(
            repo_short("file:///Users/somebody/adi-family/.adi-dev/fixtures/repos/crm-suite"),
            "…/repos/crm-suite"
        );
        // Anything else is left exactly as published — guessing at an unknown scheme would be
        // worse than showing it.
        assert_eq!(repo_short("ssh://git@host/x.git"), "ssh://git@host/x.git");
    }

    #[test]
    fn the_preview_kind_is_translated_to_the_address_spelling() {
        assert_eq!(preview_kind_dir("agent"), "agents");
        assert_eq!(preview_kind_dir("llm"), "llm", "the two spellings agree here");
        assert_eq!(preview_kind_dir("project"), "project", "and here");
        assert_eq!(preview_kind_dir("hologram"), "hologram", "unknown, passed through");
    }
}
