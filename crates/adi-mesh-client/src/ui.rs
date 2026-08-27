//! The client's own screen: a list of nodes, a way to add one, and a panel.
//!
//! Deliberately small. Everything a reader does here is on a phone, one-handed, usually to get to
//! the panel — so the shell is a list and a button, and the interesting surface is the node's own
//! control panel rendered full-bleed in an iframe (`sw.js` is what makes that iframe a page from
//! another machine).
//!
//! Three things on this screen are not decoration:
//!
//! * **This browser's key**, shown at the bottom. It is the identity of record (`docs/fleet.md`
//!   §2), it is what a node's operator sees in their fleet list, and it is the only thing a reader
//!   can quote when a node refuses them.
//! * **The petname is local.** Renaming a node here tells the node nothing (§2 rule 5); it is also
//!   the `/n/<petname>/` path its panel is served under, so it must stay one DNS label.
//! * **Removing a node forgets a password, it does not unpair.** The node still holds a record for
//!   this browser's key until its operator removes it, and the sentence on the button says so.
//!
//! The one screen that is not a list is the camera overlay, and the rule there is that it is an
//! accelerator and never a gate: **the paste field keeps working when the camera is refused,
//! absent, or pointed at nothing** ([`crate::scan`]).

use leptos::prelude::*;
use leptos::task::spawn_local;
use web_sys::HtmlVideoElement;

use crate::invite;
use crate::mesh::Mesh;
use crate::scan;
use crate::store::{self, NodeRecord};

/// What the shell is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Screen {
    /// The node list.
    Nodes,
    /// One node's control panel, in an iframe.
    Panel(String),
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

    move || match screen.get() {
        // The panel is mounted and unmounted rather than hidden: an iframe left in the tree keeps
        // its websocket and its polling alive, and a reader who went back to the list has said
        // they are done with it.
        Screen::Panel(petname) => view! { <Panel petname screen /> }.into_any(),
        Screen::Nodes => view! {
            <main class="shell">
                <header>
                    <h1>"adi"</h1>
                    <p class="sub">"Your machines, over the mesh."</p>
                </header>

                <Show when=move || !problem.get().is_empty()>
                    <p class="problem">{move || problem.get()}</p>
                </Show>

                <NodeList nodes screen ready=ready.into() />
                <AddNode nodes ready problem />

                <footer>
                    <p class="label">"this browser's key"</p>
                    <code class="key">{move || key.get()}</code>
                    <p class="note">
                        "Your key and every node password live in this browser and nowhere else. \
                         Clearing site data for this site deletes them, and you would pair again."
                    </p>
                </footer>
            </main>
        }
        .into_any(),
    }
}

/// Bind the endpoint and read what is stored. Split out so the whole start-up is one `Result`.
async fn boot() -> Result<(std::rc::Rc<Mesh>, Vec<NodeRecord>), String> {
    let secret = store::identity().await?;
    let nodes = store::nodes().await?;
    let mesh = Mesh::bind(secret).await?;
    Ok((std::rc::Rc::new(mesh), nodes))
}

#[component]
fn NodeList(
    nodes: RwSignal<Vec<NodeRecord>>,
    screen: RwSignal<Screen>,
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
        <section>
            <Show when=move || nodes.get().is_empty()>
                <p class="empty">
                    "No machines yet. On the machine you want to reach, run "
                    <code>"adi-mono mesh invite"</code>
                    " — then scan the code it draws, or paste the token below."
                </p>
            </Show>
            <ul class="nodes">
                <For each=move || nodes.get() key=|node| node.key.clone() let:node>
                    {
                        let petname = node.petname.clone();
                        let short = node.short_key();
                        move || {
                            let petname = petname.clone();
                            let short = short.clone();
                            if renaming.get() == petname {
                                let (old, gone) = (petname.clone(), petname.clone());
                                view! {
                                    <li class="editing">
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
                                view! {
                                    <li>
                                        <button
                                            class="node"
                                            disabled=move || !ready.get()
                                            on:click=move |_| screen.set(Screen::Panel(open.clone()))
                                        >
                                            <span class="name">{petname.clone()}</span>
                                            <span class="meta">{short}</span>
                                        </button>
                                        <button
                                            class="ghost"
                                            on:click=move |_| renaming.set(edit.clone())
                                        >
                                            "Edit"
                                        </button>
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
            <label for="invite">"Add a machine"</label>
            <textarea
                id="invite"
                rows="3"
                placeholder="adi-invite:…"
                autocapitalize="off"
                spellcheck="false"
                prop:value=move || token.get()
                on:input=move |ev| token.set(event_value(&ev))
            ></textarea>
            <div class="actions">
                // Absent, rather than present and inert, where there is no camera API to call —
                // over plain http, say. Everything below it still pairs a machine.
                <Show when=scan::available>
                    <button
                        id="scan"
                        class="ghost"
                        disabled=move || busy.get() || scanning.get()
                        on:click=move |_| {
                            hint.set("Starting the camera…".into());
                            scanning.set(true);
                        }
                    >
                        "Scan"
                    </button>
                </Show>
                <button class="primary" disabled=move || busy.get() || !ready.get() || token.get().trim().is_empty() on:click=move |_| pair()>
                    {move || if busy.get() { "Pairing…" } else { "Pair" }}
                </button>
            </div>
            <p class="note">
                "Run "<code>"adi-mono mesh invite"</code>" on that machine: it prints a QR code and \
                 the token itself. Scan the one or paste the other — either is good once, for \
                 fifteen minutes."
            </p>
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
                    <button class="ghost" on:click=move |_| scanning.set(false)>"Cancel"</button>
                </div>
            </div>
        </Show>
    }
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
fn Panel(petname: String, screen: RwSignal<Screen>) -> impl IntoView {
    let title = petname.clone();
    view! {
        <div class="panel">
            <div class="bar">
                <button class="back" on:click=move |_| screen.set(Screen::Nodes)>"‹ Machines"</button>
                <span class="title">{title}</span>
            </div>
            // `src` is the reserved path the service worker recognises; everything the panel loads
            // from then on is answered from the same node by client id, whatever its path.
            <iframe
                src=format!("/n/{petname}/")
                // A panel is the node's own control plane and needs its full run of the platform.
                // The one thing withheld is `allow-top-navigation`: a page from a machine should
                // not be able to replace the tab that is holding the mesh open.
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
