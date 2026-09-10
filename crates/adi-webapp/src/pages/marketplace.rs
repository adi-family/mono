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

use adi_ui::{Icon, IconSize, Lucide, Markdown};
use adi_webapp_api::types::{
    MarketplaceApp, MarketplaceInstall, MarketplaceMedia, MarketplaceMediaKind as MediaKind,
    MarketplaceSource,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, MarketplaceForm, State};
use crate::ui::{TextField, confirm, field_hint, flash_view};

/// Where a click on an app's name goes.
///
/// `Some` inside the marketplace's own door ([`market_view`]), where an app's page is another URL
/// of the same document and the click is taken over. `None` in the control panel, where it is a
/// plain navigation out to that door — the panel has no page for one app, and pretending it does
/// would be a second implementation of this screen.
type OpenApp = Option<RwSignal<String>>;

/// The marketplace door: the listing, or one app's own page when the URL names one.
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
                // A link to an app the manifest no longer lists — or a slug typed by hand. Say
                // so, and offer the one way on rather than an empty page.
                None => view! {
                    <div class="adi-empty">
                        {format!("No app called {key} in the marketplaces this machine follows.")}
                    </div>
                    {back_link(Some(open))}
                }
                .into_any(),
            }
        }}
    }
    .into_any()
}

/// The Marketplace page as the control panel draws it: the listing, and every app's name a link
/// out to the marketplace's own door.
pub(crate) fn marketplace_view(state: State, form: MarketplaceForm) -> AnyView {
    listing(state, form, None)
}

/// The listing itself: a line on what the page does with Sync beside it, then one section per
/// source with one row per app.
fn listing(state: State, form: MarketplaceForm, open: OpenApp) -> AnyView {
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

        {source_panels(state, form, open)}
    }
    .into_any()
}

/// How an entry is addressed everywhere: `<marketplace>/<slug>`. The install API's spec, the
/// busy-key of a row, and the app page's URL are all this one string.
fn app_key(app: &MarketplaceApp) -> String {
    format!("{}/{}", app.marketplace, app.slug)
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
fn source_panels(state: State, form: MarketplaceForm, open: OpenApp) -> AnyView {
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
                source_panel(state, form, source, &apps, open)
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
    open: OpenApp,
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
                    .map(|app| app_entry(state, form, app, open))
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
fn app_entry(state: State, form: MarketplaceForm, app: &MarketplaceApp, open: OpenApp) -> AnyView {
    let key = app_key(app);
    let (row_key, form_key) = (key.clone(), key.clone());
    let owned = app.clone();
    view! {
        <div class="adi-market__entry">
            {app_row(form, &owned, row_key, open)}
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
fn app_row(form: MarketplaceForm, app: &MarketplaceApp, key: String, open: OpenApp) -> AnyView {
    let (name, version, description) = (
        app.name.clone(),
        app.version.clone(),
        app.description.clone(),
    );
    let (icon, keywords) = (app_icon(app), keyword_tags(app));
    let (repo, commit) = (app.repo.clone(), short_commit(&app.commit));
    // The full strings, for the `title` of the elements that show them elided.
    let (repo_title, key_title) = (repo.clone(), key.clone());
    let button = install_button(form, app, key.clone());
    let (href, go) = (
        crate::routing::market_app_path(&app.marketplace, &app.slug),
        open_app(open, key.clone()),
    );

    view! {
        <div class="adi-market__row">
            {icon}
            <div class="adi-market__about">
                <div class="adi-market__title">
                    // The name is the way in to the app's own page — its gallery and its long
                    // form. A link and not a button, so a middle click opens it in a tab; the
                    // mark beside it is decorative and deliberately not a second tab stop.
                    <a class="adi-market__name" href=href on:click=move |ev| go(&ev)>{name}</a>
                    {version.map(|v| view! { <span class="adi-mono adi-muted">{v}</span> })}
                </div>
                {description.map(|d| view! { <div class="adi-market__desc">{d}</div> })}
                {keywords}
                <div class="adi-mono adi-muted adi-market__origin" title=repo_title>
                    {repo}" @ "{commit}
                </div>
                <div class="adi-mono adi-muted" title=key_title>{key}</div>
            </div>
            <div class="adi-market__actions">{button}</div>
        </div>
    }
    .into_any()
}

/// What a click on a link into the marketplace should do, given where this listing is drawn.
///
/// Inside the door, a plain left click is taken over: the address bar moves, the open app changes,
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

/// The way back to the listing, at the head of an app's page. The empty key is the listing, so
/// this is [`open_app`] pointed at nothing in particular.
fn back_link(open: OpenApp) -> AnyView {
    let go = open_app(open, String::new());
    view! {
        <a class="adi-market__back" href=crate::routing::MARKET on:click=move |ev| go(&ev)>
            <Icon icon=Lucide::ArrowLeft size=IconSize::Sm/>
            "All apps"
        </a>
    }
    .into_any()
}

/// The app's mark, at the head of its row: the image its manifest publishes, or a tile with the
/// package glyph when it publishes none.
///
/// A tile either way, and the same size either way, so a list of apps where only some publish an
/// icon still reads as one column of rows rather than as a ragged left edge. The image is
/// decorative — the name is right beside it — so its `alt` is empty rather than repeating the
/// name to a screen reader that has just read it.
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

/// What the entry says it is about, as tags under its description (§6 "Tag": sans, 12px, pill).
///
/// The publisher's own words and their own order — the store has already trimmed them and dropped
/// the repeats. Nothing filters by them yet; they are here because "what kind of thing is this"
/// is the question a listing of apps is asked first, and the description alone answers it one app
/// at a time.
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

/// One app's own page: what the listing row has no room for.
///
/// The order is the order somebody reads it in — what it is, what it looks like, what it does in
/// full, and only then the machine facts about what would be cloned. The install form is the same
/// one the listing opens, so there is one answer to "what will this be called" wherever it is
/// asked, and the copies already here are listed under it for the same reason.
fn app_page(
    state: State,
    form: MarketplaceForm,
    app: &MarketplaceApp,
    open: RwSignal<String>,
) -> AnyView {
    let key = app_key(app);
    let (name, version, description) = (
        app.name.clone(),
        app.version.clone(),
        app.description.clone(),
    );
    let (repo, commit) = (app.repo.clone(), short_commit(&app.commit));
    // The full strings, for the `title` of the elements that show them elided.
    let (repo_title, commit_title) = (repo.clone(), app.commit.clone());
    let branch = app.branch.clone().filter(|b| !b.trim().is_empty());
    let owned = app.clone();
    let form_key = key.clone();

    view! {
        <div class="adi-market__app">
            {back_link(Some(open))}
            <header class="adi-market__apphead">
                {app_icon_large(app)}
                <div class="adi-market__appabout">
                    <h1 class="adi-market__apptitle">
                        {name}
                        {version.map(|v| view! { <span class="adi-mono adi-muted">{v}</span> })}
                    </h1>
                    {description.map(|d| view! { <p class="adi-market__applead">{d}</p> })}
                    {keyword_tags(app)}
                </div>
                <div class="adi-market__actions">
                    {install_button(form, app, key.clone())}
                </div>
            </header>
            {flash_view(state.flash)}
            {
                let app = owned.clone();
                move || (form.installing.get() == form_key).then(|| install_form(state, form, &app))
            }
            {gallery_view(app)}
            {readme_view(app)}

            // What would actually be cloned, in mono, under everything the publisher wrote about
            // it: the listing text is theirs, and this is the part that is checkable.
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

            {(!app.installs.is_empty()).then(|| view! {
                <section class="adi-market__copies">
                    <h2 class="adi-market__copieshead">"Installed here"</h2>
                    {owned.installs.iter()
                        .map(|install| copy_row(state, form, install))
                        .collect::<Vec<_>>()}
                </section>
            })}
        </div>
    }
    .into_any()
}

/// The app's mark on its own page: the same tile as the listing's, at the size a page can afford.
fn app_icon_large(app: &MarketplaceApp) -> AnyView {
    view! { <div class="adi-market__appmark">{app_icon(app)}</div> }.into_any()
}

/// The gallery: one picture or clip at a time, with the rest as thumbnails under it.
///
/// A stage rather than a grid of squares, because these are screenshots of a working app and a
/// screenshot shrunk to a tile says nothing. The thumbnails exist only when there is more than one
/// thing to show — a strip under a single picture is a control that does nothing.
///
/// Nothing autoplays: a clip is a `<video controls>` that waits to be asked (§8 — no motion the
/// reader did not trigger), and `preload=metadata` keeps a page of clips from pulling megabytes
/// nobody asked for.
fn gallery_view(app: &MarketplaceApp) -> Option<AnyView> {
    if app.gallery.is_empty() {
        return None;
    }
    let items = app.gallery.clone();
    // Which one is on the stage. Per app page, and the page is rebuilt when the app changes, so
    // opening a second app never opens it at somebody else's third screenshot.
    let at = RwSignal::new(0usize);
    let thumbs = items.clone();
    Some(
        view! {
            <section class="adi-market__gallery">
                {move || {
                    let shown = at.get().min(items.len().saturating_sub(1));
                    items.get(shown).map(stage_item)
                }}
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

/// What is on the stage: the picture, or the player, and the publisher's line under it.
fn stage_item(item: &MarketplaceMedia) -> AnyView {
    let caption = item.caption.clone().filter(|c| !c.trim().is_empty());
    let (url, poster) = (item.url.clone(), item.poster.clone());
    view! {
        <figure class="adi-market__stage">
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
            {caption.map(|c| view! { <figcaption class="adi-market__caption">{c}</figcaption> })}
        </figure>
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
            <section class="adi-market__readme">
                <Markdown source=readme/>
            </section>
        }
        .into_any(),
    )
}

/// The button that opens the install form — on a row, and at the head of an app's page.
///
/// Not the accent, on either: pressing it opens a form, and the form's own Install is the act that
/// clones somebody else's code. That one is the screen's orange (§8).
fn install_button(form: MarketplaceForm, app: &MarketplaceApp, key: String) -> AnyView {
    let busy = form.busy;
    let again = !app.installs.is_empty();
    let default_name = app.name.clone();
    view! {
        <button class="adi-btn" type="button"
            prop:disabled=move || busy.get().is_some()
            on:click=move |_| {
                // Prefilled with the publisher's name, because it is the answer most people want
                // and the form is here to let them disagree with it.
                form.name.set(default_name.clone());
                // Starting is on by default *here* and off in the CLI, and the difference is not
                // an inconsistency: pressing Install on a page is the deliberate act, and an
                // install that leaves nothing to open reads as one that did not happen — an
                // unstarted app is filed under Archived on the Dashboards page, which is the last
                // place anybody goes looking for what they just installed. Unticking it is one
                // click for whoever wants it inert.
                form.start_now.set(true);
                form.installing.update(|open| {
                    *open = if *open == key { String::new() } else { key.clone() };
                });
            }>
            {if again { "Install another" } else { "Install" }}
        </button>
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
