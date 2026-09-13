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

use adi_ui::{Icon, IconSize, Lucide, Markdown};
use adi_webapp_api::types::{
    MarketplaceApp, MarketplaceBundleElement, MarketplaceInstall, MarketplaceMedia,
    MarketplaceMediaKind as MediaKind, MarketplaceSource,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, MarketplaceForm, State};
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
    let installed = elements.iter().filter(|el| el.id.is_some()).count();
    let outdated = app.bundle.as_ref().is_some_and(|b| b.outdated)
        || app.installs.iter().any(|i| i.outdated);
    let state = if !app.installs.is_empty() {
        // The legacy single-dashboard shape counts copies, not elements: two copies of one app is
        // ordinary there, and a fraction would be a fraction of one.
        match app.installs.len() {
            1 => "installed".to_string(),
            n => format!("{n} copies installed"),
        }
    } else if installed == 0 {
        String::new()
    } else if installed == elements.len() {
        "installed".to_string()
    } else {
        format!("{installed} of {} installed", elements.len())
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
                {hero(state, form, app)}
                {flash_view(state.flash)}
            </div>
            {gallery_view(app)}
            {assurances()}
            {included_view(state, form, app)}
            {installed_here(state, form, app)}
            {readme_view(app)}
            {facts_view(&owned)}
        </div>
    }
    .into_any()
}

/// The head of an item's page: the mark, what this is in one line above the name, the name, the
/// publisher's own sentence, its tags — and the one act the page is for.
fn hero(state: State, form: MarketplaceForm, app: &MarketplaceApp) -> AnyView {
    let key = app_key(app);
    let (name, version, description) = (
        app.name.clone(),
        app.version.clone(),
        app.description.clone(),
    );
    let owned = app.clone();
    let form_key = key.clone();

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
            <div class="adi-market__heroact">{hero_action(state, form, app)}</div>
        </header>
        {
            // The name form, for the one shape that has a name to ask about. It opens under the
            // head rather than in it, so the head does not jump when it does.
            let app = owned.clone();
            move || (form.installing.get() == form_key).then(|| install_form(state, form, &app))
        }
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
fn hero_action(state: State, form: MarketplaceForm, app: &MarketplaceApp) -> AnyView {
    let key = app_key(app);
    let busy = form.busy;
    let elements = bundle_elements(app);
    let installed = elements.iter().filter(|el| el.id.is_some()).count();
    let legacy = app.bundle.is_none() && elements.is_empty();
    let (marketplace, slug) = (app.marketplace.clone(), app.slug.clone());

    if legacy {
        let again = !app.installs.is_empty();
        let default_name = app.name.clone();
        let toggle_key = key.clone();
        return view! {
            <button class="adi-btn adi-btn--accent" type="button"
                prop:disabled=move || busy.get().is_some()
                on:click=move |_| {
                    // Prefilled with the publisher's name, because it is the answer most people
                    // want and the form is here to let them disagree with it.
                    form.name.set(default_name.clone());
                    form.start_now.set(true);
                    form.installing.update(|open| {
                        *open = if *open == toggle_key { String::new() } else { toggle_key.clone() };
                    });
                }>
                <Icon icon=Lucide::Download size=IconSize::Sm/>
                {if again { "Install another copy" } else { "Install" }}
            </button>
            <span class="adi-market__heronote">"you name your copy"</span>
        }
        .into_any();
    }

    if installed == elements.len() && !elements.is_empty() {
        return view! {
            <span class="adi-market__here">
                <span class="adi-dot adi-dot--ok"></span>
                "Everything here is installed"
            </span>
        }
        .into_any();
    }

    let label = match (installed, elements.len()) {
        (0, 1) => "Install".to_string(),
        (0, _) => "Install everything".to_string(),
        (installed, total) if total - installed == 1 => "Install the last one".to_string(),
        (installed, total) => format!("Install the other {}", total - installed),
    };
    let install_key = format!("install:{key}");
    view! {
        <button class="adi-btn adi-btn--accent" type="button"
            prop:disabled=move || busy.get().is_some()
            on:click=move |_| {
                run(state, form, install_key.clone(),
                    fetch::install_marketplace_app(
                        marketplace.clone(), slug.clone(), None, String::new(), false,
                    ));
            }>
            <Icon icon=Lucide::Download size=IconSize::Sm/>
            {label}
        </button>
        <span class="adi-market__heronote">"nothing runs until you start it"</span>
    }
    .into_any()
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

/// **What's included**: every element the item offers, one row each, with the action that applies
/// to it.
///
/// Flat and in [`KIND_ORDER`], with the kind on the row itself — not eight headings over eight
/// single rows, which is what the first version drew and which made a bundle of one of everything
/// read as an outline rather than as a list of things you can have. The section is the page's
/// middle and the reason the whole design exists: a bundle you can take one element of.
fn included_view(state: State, form: MarketplaceForm, app: &MarketplaceApp) -> Option<AnyView> {
    let elements = bundle_elements(app);
    if elements.is_empty() {
        return None;
    }
    let (marketplace, slug) = (app.marketplace.clone(), app.slug.clone());
    let installed = elements.iter().filter(|el| el.id.is_some()).count();
    let total = elements.len();
    let bundle = app.bundle.clone();
    let outdated = bundle.as_ref().is_some_and(|b| b.outdated);
    let missing_secrets = bundle.map(|b| b.missing_secrets).unwrap_or_default();
    let busy = form.busy;
    let update_key = format!("bundle-update:{marketplace}/{slug}");
    let (up_marketplace, up_slug) = (marketplace.clone(), slug.clone());

    Some(
        view! {
            <section class="adi-market__included">
                <div class="adi-market__sechead">
                    <h2 class="adi-market__sectitle">"What's included"</h2>
                    <span class="adi-market__state">
                        {if installed == 0 {
                            "install the lot above, or one element at a time".to_string()
                        } else {
                            format!("{installed} of {total} installed here")
                        }}
                    </span>
                    <span class="adi-spacer"></span>
                    {outdated.then(move || view! {
                        <button class="adi-btn" type="button"
                            prop:disabled=move || busy.get().is_some()
                            on:click=move |_| {
                                run(state, form, update_key.clone(),
                                    fetch::update_marketplace_bundle(
                                        up_marketplace.clone(), up_slug.clone(), Vec::new()));
                            }>
                            <Icon icon=Lucide::ArrowUp size=IconSize::Sm/>
                            "Update all"
                        </button>
                    })}
                </div>
                <ul class="adi-market__elements">
                    {elements.iter()
                        .map(|el| element_row(state, form, &marketplace, &slug, el))
                        .collect::<Vec<_>>()}
                </ul>
                {(!missing_secrets.is_empty()).then(|| view! {
                    <p class="adi-hint">
                        {format!(
                            "Set {} \u{2014} the elements that name it will not work until you do.",
                            missing_secrets.join(", ")
                        )}
                    </p>
                })}
            </section>
        }
        .into_any(),
    )
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

/// One element's own row: what it is, what it does, and the one act that applies to it.
///
/// **Not installed** draws Install, the row's whole point. **Installed** says so with a dot and the
/// id it landed under, and puts Uninstall — and, for a hive service, which arrives parked, Start —
/// in the row's `⋯` (§8: row actions in a menu, and never the same destructive word repeated down a
/// column). Start is idempotent on a service already started, so offering it always costs nothing.
fn element_row(
    state: State,
    form: MarketplaceForm,
    marketplace: &str,
    slug: &str,
    el: &MarketplaceBundleElement,
) -> AnyView {
    let busy = form.busy;
    let (marketplace, slug) = (marketplace.to_string(), slug.to_string());
    let spec = element_spec(&el.kind, &el.name);
    let is_service = el.kind == "services";
    let (kind, name) = (el.kind.clone(), el.name.clone());
    let description = el.description.clone();
    let landed = el.id.clone();

    let actions = match landed.clone() {
        None => {
            let install_key = format!("install:{marketplace}/{slug}/{spec}");
            let (m, s, sp) = (marketplace.clone(), slug.clone(), spec.clone());
            view! {
                <button class="adi-btn" type="button"
                    prop:disabled=move || busy.get().is_some()
                    on:click=move |_| {
                        run(state, form, install_key.clone(),
                            fetch::install_marketplace_app(
                                m.clone(), s.clone(), Some(sp.clone()), String::new(), false,
                            ));
                    }>
                    "Install"
                </button>
            }
            .into_any()
        }
        Some(id) => {
            let start_key = format!("start-service:{marketplace}/{slug}/{spec}");
            let uninstall_key = format!("uninstall:{marketplace}/{slug}/{spec}");
            let (sm, ss, sname) = (marketplace.clone(), slug.clone(), name.clone());
            let (um, us, uspec) = (marketplace.clone(), slug.clone(), spec.clone());
            let confirm_spec = spec.clone();
            let mut items = Vec::new();
            if is_service {
                items.push(menu_item(state, "Start", false, move || {
                    run(state, form, start_key.clone(),
                        fetch::start_marketplace_service(sm.clone(), ss.clone(), sname.clone()));
                }));
            }
            items.push(menu_item(state, "Uninstall", true, move || {
                if confirm(&format!(
                    "Uninstall {confirm_spec}? Its siblings in this bundle are untouched."
                )) {
                    run(state, form, uninstall_key.clone(),
                        fetch::uninstall_marketplace_element(um.clone(), us.clone(), uspec.clone()));
                }
            }));
            let inline = view! {
                <span class="adi-market__here">
                    <span class="adi-dot adi-dot--ok"></span>
                    "installed as "
                    <span class="adi-mono">{id}</span>
                </span>
            };
            row_actions(state, format!("market:{marketplace}/{slug}/{spec}"), inline, items)
        }
    };

    view! {
        <li class="adi-market__element" class:is-installed=move || landed.is_some()>
            <span class="adi-market__elementicon" aria-hidden="true">
                <Icon icon=kind_icon(&kind)/>
            </span>
            <span class="adi-market__elementabout">
                <span class="adi-market__elementname">
                    {name}
                    <span class="adi-market__kind">{kind_label(&kind).to_string()}</span>
                </span>
                {description.map(|d| view! { <span class="adi-market__elementdesc">{d}</span> })}
            </span>
            <span class="adi-spacer"></span>
            {actions}
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

/// The one question an install of the legacy shape has to ask: what to call this copy.
///
/// The name becomes the dashboard's name, its id (`Sales CRM` → `sales-crm`) and its hostname, so
/// it is worth a form rather than a guess — and the hint says out loud that it is renameable,
/// which is what makes the form cheap to answer rather than a decision to agonise over.
fn install_form(state: State, form: MarketplaceForm, app: &MarketplaceApp) -> AnyView {
    let key = app_key(app);
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
                    fetch::install_marketplace_app(marketplace, slug, None, name, start),
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
