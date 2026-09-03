//! The client's own screen: the machines this browser has paired with, what each of them runs, and
//! the one action that adds another.
//!
//! Everything a reader does here is on a phone, one-handed, and almost always to get to something
//! a machine of theirs is serving — so the shell is a list and one button, and the interesting
//! surface is that machine's own page rendered full-bleed in an iframe (`sw.js` is what makes that
//! iframe a page from another machine).
//!
//! What is on this screen, and what is deliberately not:
//!
//! * **Scanning is the primary action**, and pasting a token is behind a disclosure. A pairing
//!   token is 953 characters; the QR is how it gets onto a phone, and the field is the fallback for
//!   when the camera is refused, absent, or pointed at nothing ([`crate::scan`]). The disclosure
//!   opens itself where there is no camera API to call, so the fallback is never *hidden* — it is
//!   only quiet.
//! * **A node's dashboards are rows under it**, not a thing to go looking for behind its panel:
//!   what a machine runs is the reason to open it at all (`docs/fleet.md` §11, [`crate::dashboards`]).
//! * **This browser's key is behind a disclosure.** It is the identity of record (§2) and the only
//!   thing a reader can quote when a node refuses them — which makes it a thing to find when
//!   something is wrong, not a thing to read every time the page opens.
//! * **The petname is local.** Renaming a node here tells the node nothing (§2 rule 5); it is also
//!   the `/n/<petname>/` path its pages are served under, so it must stay one DNS label.
//! * **Removing a node forgets a password, it does not unpair.** The node still holds a record for
//!   this browser's key until its operator removes it, and the sentence on the button says so.

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use web_sys::HtmlVideoElement;

use crate::dashboards::{self, Board};
use crate::invite;
use crate::mark::Mark;
use crate::mesh::Mesh;
use crate::scan;
use crate::store::{self, NodeRecord};

/// What the shell is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Screen {
    /// The machine list.
    Nodes,
    /// One page from one machine, in an iframe.
    Page(Open),
}

/// A page being shown: what to fetch it under, and what to call it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Open {
    /// The `/n/<target>/` token — `<petname>` for a node's own panel, `<service>.<petname>` for
    /// one of its dashboards (`crate::bridge::split_target`).
    target: String,
    /// What the bar says: the dashboard's name, or the machine's.
    title: String,
    /// The machine it is running on, when the title is not already that.
    under: Option<String>,
}

/// What this browser knows about what each node runs, keyed by the node's **key** — never its
/// petname, which is local and can be renamed out from under an answer that is still arriving.
type Boards = HashMap<String, NodeBoards>;

/// One node's answer to "what do you run?".
#[derive(Debug, Clone, PartialEq, Eq)]
enum NodeBoards {
    /// Being asked. Draws nothing: a row of skeletons for a list that is usually two entries long
    /// is more movement than information.
    Asking,
    /// Answered — possibly with nothing, which is a true answer for a node that runs no dashboard.
    Answered(dashboards::Listing),
    /// The node's own sentence. Shown quietly under the machine, because it is the reason its rows
    /// are missing and it is usually also why tapping the machine itself would fail.
    Failed(String),
}

/// Mount the client.
pub fn mount() {
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let key = RwSignal::new(String::new());
    let nodes = RwSignal::new(Vec::<NodeRecord>::new());
    let screen = RwSignal::new(Screen::Nodes);
    let problem = RwSignal::new(String::new());
    let boards = RwSignal::new(Boards::new());
    // Whether the endpoint is bound. A flag and not the endpoint itself: an `Rc<Mesh>` is neither
    // `Send` nor `Sync`, and a signal is. The endpoint lives in [`crate::bridge`], which is where
    // the browser's own callbacks reach it from anyway.
    let ready = RwSignal::new(false);

    spawn_local(async move {
        match boot().await {
            Ok((bound, stored)) => {
                key.set(bound.id().to_string());
                nodes.set(stored.clone());
                crate::bridge::install(bound, stored);
                ready.set(true);
            }
            Err(e) => problem.set(e),
        }
    });

    // One place asks the nodes what they run, and it runs again whenever the list changes — after
    // the endpoint comes up, after a pairing, after a rename. Each node is asked once: a node
    // already in the map is one that has been asked, and re-asking on every render would be a
    // fresh dial per node per keystroke in the rename field.
    Effect::new(move |_| {
        if !ready.get() {
            return;
        }
        for record in nodes.get() {
            let known = boards.with_untracked(|all| all.contains_key(&record.key));
            if !known {
                survey(record, boards);
            }
        }
    });

    move || {
        match screen.get() {
        // The page is mounted and unmounted rather than hidden: an iframe left in the tree keeps
        // its websocket and its polling alive, and a reader who went back to the list has said
        // they are done with it.
        Screen::Page(open) => view! { <Page open screen /> }.into_any(),
        Screen::Nodes => view! {
            <main class="shell">
                <header>
                    <Mark />
                    <div>
                        <h1>"adi"</h1>
                        <p class="sub">"Your machines, over the mesh."</p>
                    </div>
                </header>

                <Show when=move || !problem.get().is_empty()>
                    <p class="problem">{move || problem.get()}</p>
                </Show>

                <NodeList nodes screen boards ready=ready.into() />
                <AddNode nodes ready problem />

                <footer>
                    // Closed by default. What is in here is what a reader needs when a node
                    // refuses them, and nothing at all on the days it does not.
                    <details>
                        <summary><span inner_html=icon(CHEVRON_RIGHT, 14)></span>"This browser"</summary>
                        <p class="label">"This browser's key"</p>
                        <code class="key">{move || key.get()}</code>
                        <p class="note">
                            "Your key and every node password live in this browser and nowhere \
                             else. Clearing site data for this site deletes them, and you would \
                             pair again."
                        </p>
                    </details>
                </footer>
            </main>
        }
        .into_any(),
    }
    }
}

/// Bind the endpoint and read what is stored. Split out so the whole start-up is one `Result`.
async fn boot() -> Result<(std::rc::Rc<Mesh>, Vec<NodeRecord>), String> {
    let secret = store::identity().await?;
    let nodes = store::nodes().await?;
    let mesh = Mesh::bind(secret).await?;
    Ok((std::rc::Rc::new(mesh), nodes))
}

/// Ask one node what it runs, and file the answer under its key.
///
/// A task per node rather than one that walks the list: each is a dial to a different machine over
/// a relay, and a phone with one sleeping laptop in the list would otherwise wait out that node's
/// timeout before hearing from any of the others.
fn survey(record: NodeRecord, boards: RwSignal<Boards>) {
    let key = record.key.clone();
    boards.update(|all| {
        all.insert(key.clone(), NodeBoards::Asking);
    });
    spawn_local(async move {
        let Some(mesh) = crate::bridge::endpoint() else {
            return;
        };
        let answer = match dashboards::list(&mesh, &record).await {
            Ok(listing) => NodeBoards::Answered(listing),
            Err(e) => NodeBoards::Failed(e),
        };
        // `try_update`: the shell may have gone — a reader who opened a panel while a slow node
        // was still answering has unmounted every signal this task holds.
        boards.try_update(|all| {
            all.insert(key, answer);
        });
    });
}

#[component]
fn NodeList(
    nodes: RwSignal<Vec<NodeRecord>>,
    screen: RwSignal<Screen>,
    boards: RwSignal<Boards>,
    ready: Signal<bool>,
) -> impl IntoView {
    let renaming = RwSignal::new(String::new());

    let rename = move |old: String, wanted: String| {
        spawn_local(async move {
            let wanted = wanted.trim().to_lowercase();
            if !crate::protocol::is_dns_label(&wanted) {
                return;
            }
            let mut all = nodes.get_untracked();
            let free = store::free_petname(
                &all.iter()
                    .filter(|n| n.petname != old)
                    .cloned()
                    .collect::<Vec<_>>(),
                &wanted,
            );
            if let Some(record) = all.iter_mut().find(|n| n.petname == old) {
                record.petname = free;
            }
            if store::save_nodes(&all).await.is_ok() {
                crate::bridge::set_nodes(all.clone());
                nodes.set(all);
            }
            renaming.set(String::new());
        });
    };

    let remove = move |petname: String| {
        spawn_local(async move {
            let all: Vec<NodeRecord> = nodes
                .get_untracked()
                .into_iter()
                .filter(|n| n.petname != petname)
                .collect();
            if store::save_nodes(&all).await.is_ok() {
                crate::bridge::set_nodes(all.clone());
                nodes.set(all);
            }
        });
    };

    view! {
        <section class="machines">
            <Show when=move || nodes.get().is_empty()>
                <p class="empty">
                    "No machines yet. On the machine you want to reach, run "
                    <code>"adi-mono mesh invite"</code>
                    " — then scan the code it draws."
                </p>
            </Show>
            <ul class="nodes">
                <For each=move || nodes.get() key=|node| node.key.clone() let:node>
                    {
                        let record = node.clone();
                        let petname = node.petname.clone();
                        let short = node.short_key();
                        move || {
                            let record = record.clone();
                            let petname = petname.clone();
                            let short = short.clone();
                            if renaming.get() == petname {
                                let (old, gone) = (petname.clone(), petname.clone());
                                view! {
                                    <li class="machine editing">
                                        <input
                                            class="rename"
                                            value=petname.clone()
                                            autocapitalize="off"
                                            spellcheck="false"
                                            on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                                                if ev.key() == "Enter" {
                                                    rename(old.clone(), event_value(&ev));
                                                } else if ev.key() == "Escape" {
                                                    renaming.set(String::new());
                                                }
                                            }
                                        />
                                        <button
                                            class="danger"
                                            title="Forgets this node's password here. The node \
                                                   still holds a record for this browser until \
                                                   its operator removes it."
                                            on:click=move |_| remove(gone.clone())
                                        >
                                            "Forget"
                                        </button>
                                    </li>
                                }
                                .into_any()
                            } else {
                                let (open, edit) = (petname.clone(), petname.clone());
                                let title = petname.clone();
                                view! {
                                    <li class="machine">
                                        <div class="row">
                                            <button
                                                class="node"
                                                disabled=move || !ready.get()
                                                on:click=move |_| {
                                                    screen.set(Screen::Page(Open {
                                                        target: open.clone(),
                                                        title: title.clone(),
                                                        under: None,
                                                    }));
                                                }
                                            >
                                                <span class="name">{petname.clone()}</span>
                                                <span class="meta">{short}</span>
                                            </button>
                                            <button
                                                class="more"
                                                aria-label="Rename or forget this machine"
                                                on:click=move |_| renaming.set(edit.clone())
                                                inner_html=icon(ELLIPSIS, 16)
                                            ></button>
                                        </div>
                                        <Boards record screen boards />
                                    </li>
                                }
                                .into_any()
                            }
                        }
                    }
                </For>
            </ul>
        </section>
    }
}

/// The dashboards one machine runs, under its row.
///
/// A row that this browser holds no grant for is drawn as an offer rather than as a link that
/// would answer *not authorized*: pairing grants `http:app` and nothing else, so the first tap on
/// such a row asks the node for `http:<service>` and opens it once the node has taken it up
/// (`docs/fleet.md` §11, [`dashboards::allow`]). That escalates nothing — this browser already
/// holds the node's control panel, which *is* the thing that hands out grants.
#[component]
fn Boards(record: NodeRecord, screen: RwSignal<Screen>, boards: RwSignal<Boards>) -> impl IntoView {
    // The service currently being asked for, so its own row can say so. One at a time, because it
    // is one tap: the node's five-second reload is what it costs, and a second tap during it lands
    // on a row that is already busy.
    let asking = RwSignal::new(String::new());
    let key = record.key.clone();

    let open = move |board: Board, me: Option<String>| {
        let record = record.clone();
        let target = format!("{}.{}", board.service, record.petname);
        let show = Screen::Page(Open {
            target,
            title: board.name.clone(),
            under: Some(record.petname.clone()),
        });
        if board.granted {
            screen.set(show);
            return;
        }
        if !asking.get_untracked().is_empty() {
            return;
        }
        asking.set(board.service.clone());
        spawn_local(async move {
            let Some(mesh) = crate::bridge::endpoint() else {
                return;
            };
            match dashboards::allow(&mesh, &record, me.as_deref(), &board.service).await {
                Ok(()) => {
                    // Remembered on the row rather than re-listed: the node has just told us it
                    // holds the grant, and a second round trip to be told again is a second wait.
                    boards.try_update(|all| {
                        if let Some(NodeBoards::Answered(listing)) = all.get_mut(&record.key) {
                            for row in &mut listing.boards {
                                if row.service == board.service {
                                    row.granted = true;
                                }
                            }
                        }
                    });
                    asking.try_set(String::new());
                    screen.try_set(show);
                }
                Err(e) => {
                    boards.try_update(|all| {
                        all.insert(record.key.clone(), NodeBoards::Failed(e));
                    });
                    asking.try_set(String::new());
                }
            }
        });
    };

    move || match boards.get().get(&key).cloned() {
        None | Some(NodeBoards::Asking) => ().into_any(),
        Some(NodeBoards::Failed(why)) => view! { <p class="quiet">{why}</p> }.into_any(),
        Some(NodeBoards::Answered(listing)) => {
            let me = listing.me.clone();
            // A clone per render, because the `<For>` body owns what it uses and this closure has
            // to stay callable for the next one.
            let open = open.clone();
            view! {
                <ul class="boards">
                    <For
                        each=move || listing.boards.clone()
                        key=|board| (board.service.clone(), board.granted)
                        let:board
                    >
                        {
                            let (row, service) = (board.clone(), board.service.clone());
                            let me = me.clone();
                            let open = open.clone();
                            view! {
                                <li>
                                    <button
                                        class="board"
                                        disabled=move || {
                                            !asking.get().is_empty() && asking.get() != service
                                        }
                                        on:click=move |_| open(row.clone(), me.clone())
                                    >
                                        {dot(board.clone(), asking)}
                                        <span class="name">{board.name.clone()}</span>
                                        {word(board.clone(), asking)}
                                    </button>
                                </li>
                            }
                        }
                    </For>
                </ul>
            }
            .into_any()
        }
    }
}

/// The 6px dot before a dashboard that is up and open to this browser — the state that says
/// nothing else (`design/DESIGN.md` §6: a dot, never a badge).
fn dot(board: Board, asking: RwSignal<String>) -> impl IntoView {
    let service = board.service;
    let live = board.granted && board.running;
    move || (live && asking.get() != service).then(|| view! { <span class="dot"></span> })
}

/// The one word beside a dashboard's name, or nothing.
///
/// Nothing is the common case and the point: a dashboard that is running and granted is a row with
/// a dot and a name on it. The words that do appear are the two things a reader would otherwise
/// learn by tapping and waiting — that the node will have to be asked first, and that the dashboard
/// is not up over there.
fn word(board: Board, asking: RwSignal<String>) -> impl IntoView {
    let service = board.service;
    let (granted, running) = (board.granted, board.running);
    move || {
        if asking.get() == service {
            return Some(view! { <span class="word">"Asking…"</span> });
        }
        if !granted {
            return Some(view! { <span class="word">"Tap to allow"</span> });
        }
        (!running).then(|| view! { <span class="word">"Not running"</span> })
    }
}

#[component]
fn AddNode(
    nodes: RwSignal<Vec<NodeRecord>>,
    ready: RwSignal<bool>,
    problem: RwSignal<String>,
) -> impl IntoView {
    let token = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    // The camera overlay: whether it is up, and the one line of words under the picture. `hint`
    // is deliberately separate from `problem` — a scanner that has not found anything yet is not
    // an error on the page behind it, and it disappears with the overlay.
    let scanning = RwSignal::new(false);
    let hint = RwSignal::new(String::new());
    let camera: NodeRef<leptos::html::Video> = NodeRef::new();

    let pair = move || {
        if busy.get_untracked() {
            return;
        }
        let Some(mesh) = crate::bridge::endpoint() else {
            problem.set("the mesh endpoint is still starting — try again in a moment".into());
            return;
        };
        let raw = token.get_untracked();
        busy.set(true);
        problem.set(String::new());
        spawn_local(async move {
            match pair_with(&mesh, &raw).await {
                Ok(record) => match store::add_node(record).await {
                    Ok(all) => {
                        crate::bridge::set_nodes(all.clone());
                        nodes.set(all);
                        token.set(String::new());
                    }
                    Err(e) => problem.set(e),
                },
                Err(e) => problem.set(e),
            }
            busy.set(false);
        });
    };

    // A scanning session lives exactly as long as the `<video>` does. It is started from an effect
    // on the element and not from the click on Scan, because a stream cannot be attached to an
    // element that is not in the document yet — and because the effect re-runs when the reference
    // changes, which is what makes a *second* scan after a Cancel work rather than open a dead
    // camera. `spawn_local` and not a `Resource`: this is a loop with a camera on the end of it,
    // not a value the view waits for.
    Effect::new(move |_| {
        let Some(video) = camera.get() else { return };
        spawn_local(scan_session(video, scanning, hint, token, pair));
    });

    view! {
        <section class="add">
            // Absent, rather than present and inert, where there is no camera API to call — over
            // plain http, say. The disclosure below opens itself in that case, so the flow that
            // always works is the one on screen.
            <Show when=scan::available>
                <button
                    id="scan"
                    class="cta"
                    disabled=move || busy.get() || scanning.get() || !ready.get()
                    on:click=move |_| {
                        hint.set("Starting the camera…".into());
                        scanning.set(true);
                    }
                >
                    <span class="glyph" inner_html=icon(SCAN_LINE, 20)></span>
                    "Scan a pairing code"
                </button>
            </Show>

            <details class="paste" open=!scan::available()>
                <summary>"Paste a token instead"</summary>
                <textarea
                    id="invite"
                    rows="3"
                    placeholder="adi-invite:…"
                    autocapitalize="off"
                    spellcheck="false"
                    prop:value=move || token.get()
                    on:input=move |ev| token.set(event_value(&ev))
                ></textarea>
                <button
                    class="strong"
                    disabled=move || {
                        busy.get() || !ready.get() || token.get().trim().is_empty()
                    }
                    on:click=move |_| pair()
                >
                    {move || if busy.get() { "Pairing…" } else { "Pair" }}
                </button>
                <p class="note">
                    "Run "<code>"adi-mono mesh invite"</code>" on that machine: it prints a QR code \
                     and the token itself. Either is good once, for fifteen minutes."
                </p>
            </details>
        </section>

        <Show when=move || scanning.get()>
            <div class="scanner">
                // `playsinline` and `muted` are set in `scan::open`, not here: the `muted`
                // *attribute* maps to `defaultMuted` and would leave an element the DOM built at
                // runtime unmuted — and an unmuted video may not autoplay. The click is the retry
                // for an engine that refused to start the picture on its own.
                <video
                    node_ref=camera
                    on:click=move |_| {
                        if let Some(video) = camera.get_untracked() {
                            let _ = video.play();
                        }
                    }
                ></video>
                <div class="reticle"></div>
                <div class="scanbar">
                    <p class="hint">{move || hint.get()}</p>
                    <button class="ghost" on:click=move |_| scanning.set(false)>
                        "Cancel"
                    </button>
                </div>
            </div>
        </Show>
    }
}

// The icons are Lucide, the one set the product uses (`design/DESIGN.md` §9), read out of the
// shared directory at compile time. Inlined rather than fetched, for the reason [`crate::mark`]
// is: this client fetches nothing from anywhere, and an icon file would be a second request on a
// cold phone.
const ELLIPSIS: &str = include_str!("../../adi-ui/icons/ellipsis.svg");
const SCAN_LINE: &str = include_str!("../../adi-ui/icons/scan-line.svg");
const CHEVRON_LEFT: &str = include_str!("../../adi-ui/icons/chevron-left.svg");
const CHEVRON_RIGHT: &str = include_str!("../../adi-ui/icons/chevron-right.svg");

/// One icon as markup: the paths from a Lucide file, in the frame §9 fixes — stroke 1.5, one of
/// the four sizes, `currentColor` so it takes the ink of the text beside it.
///
/// The file is kept verbatim on disk (its licence header travels with the paths); only what sits
/// between `<svg …>` and `</svg>` is drawn, because the wrapper is where the weight and the size
/// are decided, and Lucide's own is stroke 2 and 24px.
fn icon(svg: &'static str, size: u32) -> String {
    let start = svg
        .find("<svg")
        .and_then(|at| svg[at..].find('>').map(|end| at + end + 1))
        .unwrap_or(0);
    let end = svg.rfind("</svg>").unwrap_or(svg.len());
    format!(
        r#"<svg width="{size}" height="{size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{}</svg>"#,
        &svg[start..end]
    )
}

/// Hold the camera open until it reads an invite, or until the reader gives up.
///
/// The whole session is one task and one owner of the stream, which is what makes "stop the camera
/// on every path out" a thing that can be read rather than a thing to remember: every exit falls
/// through to the same [`scan::stop`]. Cancelling flips `scanning`, and the loop notices within a
/// frame — cheaper than a channel, and it also ends the session if the shell unmounts underneath
/// it, because a disposed signal reads as `None`.
///
/// A code that is not an invite does **not** end the session. Somebody pointing a phone across a
/// desk will sweep past a parcel label, and a scanner that quit on one would be worse than useless.
async fn scan_session<P: Fn()>(
    video: HtmlVideoElement,
    scanning: RwSignal<bool>,
    hint: RwSignal<String>,
    token: RwSignal<String>,
    pair: P,
) {
    let mut reader = match scan::Reader::new() {
        Ok(reader) => reader,
        Err(e) => return hint.set(e),
    };
    // The overlay stays up on a refusal, holding the sentence that says why. Cancel is the way out
    // of it, and the paste field is behind it either way.
    let stream = match scan::open(&video).await {
        Ok(stream) => stream,
        Err(e) => return hint.set(e),
    };

    let started = crate::now_ms();
    let mut wrong_code = false;
    // Cancelled while the permission prompt was up: the loop is skipped and the camera that has
    // just been granted is handed straight back.
    while scanning.try_get().unwrap_or(false) {
        match reader.read(&video) {
            Ok(Some(text)) => {
                // Checked with the decoder that will spend it, so anything reaching the field is
                // an invite this build can actually read. The text itself is never logged or shown
                // — it is a live credential for somebody's machine.
                if invite::decode_invite(&text).is_ok() {
                    scan::stop(&stream, &video);
                    token.set(text);
                    scanning.set(false);
                    pair();
                    return;
                }
                wrong_code = true;
                hint.set(
                    "That is a QR code, but not an adi invite. Run `adi-mono mesh invite` on the \
                     machine you want to reach."
                        .into(),
                );
            }
            Ok(None) => {
                if !wrong_code {
                    let next = waiting(&video, started);
                    // Only when it changes: this runs eight times a second, and re-rendering the
                    // same sentence would keep the overlay busy for nothing.
                    if hint.with_untracked(|shown| shown != next) {
                        hint.set(next.to_string());
                    }
                }
            }
            Err(e) => {
                hint.set(e);
                break;
            }
        }
        n0_future::time::sleep(scan::FRAME_INTERVAL).await;
    }
    scan::stop(&stream, &video);
}

/// What to say while nothing has been found yet.
///
/// Three different situations, and the difference between them is the whole value of the line: a
/// camera that never started is somebody's autoplay policy, a camera that is running and sees
/// nothing is somebody's aim, and neither of them is worth a spinner.
fn waiting(video: &HtmlVideoElement, started_ms: f64) -> &'static str {
    let waited = crate::now_ms() - started_ms;
    if !scan::has_frame(video) {
        if waited > 4000.0 {
            "The camera has not started. Tap the picture, or use the paste field instead."
        } else {
            "Starting the camera…"
        }
    } else if waited > 8000.0 {
        "Nothing yet. Fill the frame with the code and hold still — or paste the token instead."
    } else {
        "Point this at the QR code your machine is showing."
    }
}

/// Spend an invite and turn what comes back into a record for this browser.
async fn pair_with(mesh: &Mesh, token: &str) -> Result<NodeRecord, String> {
    let invite = invite::decode_invite(token)?;
    let nickname = invite::nickname_for(mesh.id());
    let (addr, accepted) = invite::join(mesh, &invite, &nickname).await?;
    // The node accepted, but its gateway has not re-read its own registry yet — see
    // [`invite::wait_until_admitted`]. Paying that wait here is what makes the row, when it
    // appears, a row that opens.
    invite::wait_until_admitted(mesh, &addr, crate::bridge::PANEL_SERVICE).await?;

    // §2 rule 5: the petname is *this* machine's name for the node, so it is chosen here. The
    // `petname` in the reply is what the node decided to call this browser, which is a different
    // fact and belongs on the other side's screen.
    let existing = store::nodes().await?;
    let wanted = format!("node-{}", addr.id.fmt_short());
    Ok(NodeRecord {
        petname: store::free_petname(&existing, &wanted),
        key: addr.id.to_string(),
        // The relay the node named in its own ticket. Not this client's home relay: a node need
        // not be on the same one, and carrying it per node is what lets a client served from one
        // domain reach a fleet spread over several (`docs/fleet.md` §9).
        relay: relay_of(&addr),
        username: accepted.username,
        password: accepted.password,
        paired_at: unix_seconds(),
        grants: accepted.grants,
    })
}

/// Now, in whole unix seconds.
///
/// `Date::now()` is milliseconds since the epoch as an `f64`, which is exact for every instant this
/// century — the cast loses only the fraction of a second the field never wanted.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "an f64 holds every millisecond of this century exactly; the seconds are whole and \
              positive by the time they are cast"
)]
fn unix_seconds() -> u64 {
    (js_sys::Date::now() / 1000.0).max(0.0).trunc() as u64
}

/// The relay URL in an address, if it carries one.
fn relay_of(addr: &iroh::EndpointAddr) -> String {
    addr.addrs
        .iter()
        .find_map(|a| match a {
            iroh::TransportAddr::Relay(url) => Some(url.to_string()),
            // An IP hint, or a transport a later iroh adds. Neither is reachable from a browser:
            // there is no UDP here, so the relay is the only address that can ever matter.
            _ => None,
        })
        .unwrap_or_default()
}

#[component]
fn Page(open: Open, screen: RwSignal<Screen>) -> impl IntoView {
    let Open {
        target,
        title,
        under,
    } = open;
    view! {
        <div class="panel">
            <div class="bar">
                <button class="back" on:click=move |_| screen.set(Screen::Nodes)>
                    <span inner_html=icon(CHEVRON_LEFT, 16)></span>
                    "Back"
                </button>
                <span class="title">
                    {title}
                    {under.map(|node| view! { <span class="under">{node}</span> })}
                </span>
            </div>
            // `src` is the reserved path the service worker recognises; everything the page loads
            // from then on is answered from the same service by client id, whatever its path.
            <iframe
                src=format!("/n/{target}/")
                // A node's own page needs its full run of the platform. The one thing withheld is
                // `allow-top-navigation`: a page from a machine should not be able to replace the
                // tab that is holding the mesh open.
                sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-modals allow-downloads"
            ></iframe>
        </div>
    }
}

/// The text in whatever input raised `event`.
fn event_value(event: &leptos::ev::Event) -> String {
    use wasm_bindgen::JsCast as _;
    event
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|input| input.value())
        .or_else(|| {
            event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
                .map(|area| area.value())
        })
        .unwrap_or_default()
}
