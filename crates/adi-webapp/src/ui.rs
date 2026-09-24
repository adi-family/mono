//! Shared view helpers, formatters, and the generic mutation runner the pages compose from.

use adi_webapp_api::types::{AgentRunInfo, ProcessUsage, ServicePort, TaskRow, TestResultDto};
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::state::{Flash, RowMenu, State};

/// Tables live in [`adi_ui`]; re-exported under the names the pages import. `SortKey` comes back
/// as `Key` — unqualified it is too vague to export from a component library, and too long to
/// write 115 times in a comparator.
pub(crate) use adi_ui::{Sort, SortKey as Key, TableState, sort_rows};

/// The two placeholders a sortable table body opens with: `Loading…` while the page's data signal
/// is still empty, then `empty` once it has landed with nothing in it. `Ok` carries the rows to
/// sort and render — the part that actually differs between tables.
///
/// Written out at every table body it looked like boilerplate; it is a house rule. A table that
/// skipped the first placeholder would flash its empty line during the load, and one that skipped
/// the second would render a bare frame saying nothing about why it is bare.
///
/// A table whose read can fail wants [`rows_or_status`] instead: `Loading…` is a lie once the read
/// has come back and come back badly, and it is a lie that never expires.
///
/// # Errors
/// The placeholder view to render instead of rows.
pub(crate) fn rows_or_placeholder<T>(
    table: TableState,
    loaded: Option<Vec<T>>,
    empty: &str,
) -> Result<Vec<T>, AnyView> {
    rows_or_status(table, loaded, empty, None)
}

/// [`rows_or_placeholder`], plus the third thing a table body can be: a read that failed.
///
/// `error` is why the endpoint behind these rows last refused to answer — [`crate::state::State`]'s
/// `read_errors`, looked up by the path. It wins over `Loading…`, because the two are told apart by
/// nothing else: both show as an empty signal, and a page that cannot load has no later state to
/// move to. It does **not** win over rows that did load, so a list already on screen survives a
/// failed refresh rather than being replaced by the reason for it.
///
/// # Errors
/// The placeholder view to render instead of rows.
pub(crate) fn rows_or_status<T>(
    table: TableState,
    loaded: Option<Vec<T>>,
    empty: &str,
    error: Option<String>,
) -> Result<Vec<T>, AnyView> {
    let Some(rows) = loaded else {
        let said = error.map_or_else(
            || "Loading…".to_string(),
            |error| format!("Couldn't load this: {error}"),
        );
        return Err(view! { <adi_ui::EmptyRow state=table>{said}</adi_ui::EmptyRow> }.into_any());
    };
    if rows.is_empty() {
        let empty = empty.to_string();
        return Err(view! { <adi_ui::EmptyRow state=table>{empty}</adi_ui::EmptyRow> }.into_any());
    }
    Ok(rows)
}

/// A live pane's input row: a text field (submit types it into the session, without a trailing
/// Enter — the ⏎ quick key sends that) plus the special keys interactive programs need.
///
/// Shared by the agents live view and the workspace terminal, which differ only in where the
/// typed text goes. `send(text, key)` types `text` literally and then presses `key`; either may
/// be empty, and the callee decides what an empty pair means.
pub(crate) fn send_bar(
    input: RwSignal<String>,
    placeholder: &'static str,
    send: impl Fn(String, &'static str) + Copy + 'static,
) -> impl IntoView {
    view! {
        <form class="adi-form"
            on:submit=move |ev| {
                ev.prevent_default();
                let text = input.get();
                input.set(String::new());
                send(text, "");
            }>
            <input class="adi-input adi-input--wide adi-mono" autocomplete="off"
                placeholder=placeholder
                prop:value=move || input.get()
                on:input=move |ev| input.set(event_target_value(&ev)) />
            <button class="adi-btn adi-btn--primary" type="submit">"Send"</button>
            {quick_key("⏎", "Enter", send)}
            {quick_key("↑", "Up", send)}
            {quick_key("↓", "Down", send)}
            {quick_key("Tab", "Tab", send)}
            {quick_key("Esc", "Escape", send)}
            {quick_key("^C", "C-c", send)}
        </form>
    }
}

/// One special-key button in [`send_bar`], pressing a single key in the session.
fn quick_key(
    label: &'static str,
    key: &'static str,
    send: impl Fn(String, &'static str) + Copy + 'static,
) -> impl IntoView {
    view! {
        <button class="adi-btn adi-btn--ghost adi-mono" type="button"
            title=format!("send {key}")
            on:click=move |_| send(String::new(), key)>{label}</button>
    }
}

/// A live pane's `<pre>`, held to one element for as long as `text` streams rather than rebuilt
/// with the surrounding view — the same fix as `pages::agents::actions::pty_pane` (its twin,
/// read that doc first), generalised off `AgentsWatch` to any text source so the workspace
/// terminal and the hook/trigger log panels can share it instead of copying the pattern three
/// times: `text` is read only inside the reactive child below (and, to know when to re-check the
/// scroll position, inside the effect that follows it down), never by the caller's own `view!`,
/// so a poll landing new text updates this element's text node in place instead of remounting it
/// and resetting its scroll to the top.
///
/// Scroll is pinned to the bottom only while the reader was already there — checked off the
/// pane's own scroll position on every scroll they make, not remembered from an earlier poll —
/// so a reader who has scrolled up to read earlier output is left exactly where they are.
pub(crate) fn log_pane(
    class: &'static str,
    text: impl Fn() -> String + Clone + Send + 'static,
) -> impl IntoView {
    let el: NodeRef<html::Pre> = NodeRef::new();
    let pinned = RwSignal::new(true);
    let on_scroll = move |_| {
        if let Some(el) = el.get_untracked() {
            let gap = el.scroll_height() - el.client_height() - el.scroll_top();
            pinned.set(gap <= 4);
        }
    };
    let watched_text = text.clone();
    Effect::new(move |_| {
        // Tracked only to rerun this effect on every poll — the text itself is read (and
        // written to the DOM) by the reactive child below.
        watched_text();
        let Some(el) = el.get_untracked() else { return };
        if !pinned.get_untracked() {
            return;
        }
        // A timeout, not scrolled here directly: the text this effect depends on has not
        // repainted yet at this point in the reactive graph, so `scroll_height` read now would
        // still describe the pane from before the new output landed.
        gloo_timers::callback::Timeout::new(0, move || el.set_scroll_top(el.scroll_height()))
            .forget();
    });
    view! {
        <pre class=class node_ref=el on:scroll=on_scroll>
            {move || text()}
        </pre>
    }
}

/// The three states a polled log view moves through, coarsened from "is there a snapshot yet,
/// and did the thing ever run" so a poll that only lands more output leaves the phase — and so
/// the chrome around [`log_pane`] — untouched. Shared by the hook-log and trigger-log panels;
/// the workspace terminal has its own three-way phase (`pages::workspaces::TermPhase`) since a
/// live pty session distinguishes Connecting from Ended, which a plain run log never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogPhase {
    /// No snapshot has landed yet.
    Loading,
    /// A snapshot landed, but the thing it logs has never run.
    Empty,
    /// A snapshot with a body to show, via [`log_pane`].
    Ready,
}

impl LogPhase {
    /// Derive the phase from "has a snapshot landed" (`None` while it hasn't) crossed with "did
    /// the thing it logs ever run" (a hook's `ran`, a trigger's `fired`).
    pub(crate) fn from_ran(ran: Option<bool>) -> Self {
        match ran {
            None => Self::Loading,
            Some(false) => Self::Empty,
            Some(true) => Self::Ready,
        }
    }
}

/// A full-width placeholder row spanning `colspan` columns.
///
/// For the SQL console's result grid, which has no [`TableState`] to take a span from. Tables
/// that do have one use [`adi_ui::EmptyRow`], which reads the span off the layout itself.
pub(crate) fn placeholder_row(colspan: usize, msg: &str) -> AnyView {
    view! { <tr><td class="adi-empty" colspan=colspan>{msg.to_string()}</td></tr> }.into_any()
}

/// The trailing action controls for a table row: any always-visible `inline` buttons, then — when
/// `items` is non-empty — a `⋯` opening an overflow menu of them (each built with
/// [`menu_item`]). `key` identifies this row's menu and must be unique among the rows on screen
/// (namespace it per table, e.g. `secret:…`/`tool:…`, so two panels on one page never collide).
/// Backed by the single [`State::row_menu`] signal, so only one menu is ever open.
pub(crate) fn row_actions(
    state: State,
    key: String,
    inline: impl IntoView + 'static,
    items: Vec<AnyView>,
) -> AnyView {
    if items.is_empty() {
        return view! { <div class="adi-rowacts">{inline}</div> }.into_any();
    }
    let rm = state.row_menu;
    let toggle_key = key.clone();
    let aria_key = key.clone();
    // The menu stays mounted but positionless until this row is the open one, so each item's
    // click handler can stay a plain move-closure instead of something rebuildable — which is
    // what [`adi_ui::Menu`]'s `at: Option` is for.
    view! {
        <div class="adi-rowacts">
            {inline}
            <button class="adi-btn adi-btn--icon-sm" type="button" title="More actions"
                aria-label="More actions"
                aria-expanded=move || rm.get().is_some_and(|m| m.key == aria_key).to_string()
                on:click=move |ev: web_sys::MouseEvent| toggle_row_menu(rm, &toggle_key, &ev)>
                <adi_ui::Icon icon=adi_ui::Lucide::Ellipsis/>
            </button>
        </div>
        <adi_ui::Menu
            at=move || match rm.get() {
                Some(m) if m.key == key => Some(adi_ui::MenuAt::RightOf(m.right, m.top)),
                _ => None,
            }
            on_dismiss=Callback::new(move |()| rm.set(None))>
            {items}
        </adi_ui::Menu>
    }
    .into_any()
}

/// One item in a row's overflow [menu](row_actions): a full-width menu button that closes the menu,
/// then runs `on_select`. `danger` tints it as destructive (Remove/Delete). Wrap the call in a
/// `.then(|| …)` to make an item conditional.
pub(crate) fn menu_item(
    state: State,
    label: &str,
    danger: bool,
    on_select: impl Fn() + Send + Sync + 'static,
) -> AnyView {
    let rm = state.row_menu;
    let label = label.to_string();
    view! {
        <adi_ui::MenuItem danger=danger
            on_select=Callback::new(move |()| { rm.set(None); on_select(); })>
            {label}
        </adi_ui::MenuItem>
    }
    .into_any()
}

/// Open (or close, if already this row's) the shared row menu for `key`, anchored to the click
/// point. Anchored from the viewport's right edge so it opens leftward from the right-aligned ⋯
/// and never spills off-screen.
fn toggle_row_menu(rm: RwSignal<Option<RowMenu>>, key: &str, ev: &web_sys::MouseEvent) {
    if rm.get_untracked().is_some_and(|m| m.key == key) {
        rm.set(None);
        return;
    }
    let inner_w = web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    #[allow(clippy::cast_possible_truncation)]
    rm.set(Some(RowMenu {
        key: key.to_string(),
        right: (inner_w - f64::from(ev.client_x())) as i32,
        top: ev.client_y(),
    }));
}

/// Format an uptime in seconds as `Ns` / `Nm Ss` / `Nh Mm`.
pub(crate) fn fmt_uptime(s: u64) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s < 3_600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3_600, (s % 3_600) / 60)
    }
}

/// The one-line failure shown under a form, with the × that takes it down.
pub(crate) fn flash_view(flash: RwSignal<Option<Flash>>) -> impl IntoView {
    flash_line("adi-flash", flash)
}

/// [`flash_view`] in its standing form: the notice above a page's content, with a hairline under
/// it (§6). For the frames that show the flash before the page rather than inside a form — the
/// chat home, Mesh, Fleet.
pub(crate) fn flash_card(flash: RwSignal<Option<Flash>>) -> impl IntoView {
    flash_line("adi-flash adi-flash--card", flash)
}

/// Both forms of the failure line. Nothing renders while there is no failure — not an empty box
/// holding its height, which in the card form would draw its hairline under a page that has
/// nothing to say.
///
/// The × is load-bearing, not decoration: nothing else clears [`Flash`] but the next mutation on
/// the same page, so without it a failure that has been read and understood stays on screen for
/// as long as the reader stays there.
fn flash_line(class: &'static str, flash: RwSignal<Option<Flash>>) -> impl IntoView {
    view! {
        {move || flash.get().map(|f| view! {
            <div class=class>
                <span class="adi-flash__msg">{f.msg}</span>
                <button class="adi-flash__close" type="button" aria-label="Dismiss"
                    on:click=move |_| flash.set(None)>
                    <adi_ui::Icon icon=adi_ui::Lucide::X size=adi_ui::IconSize::Sm/>
                </button>
            </div>
        })}
    }
}

/// The last on-demand **Test**'s verdict, beside the button that ran it — shared by the LLM and
/// embedding backend editors, since both send a real request through the form as it stands and
/// both answer with the same [`TestResultDto`] shape.
///
/// Green for an answer (the `adi-status` pill every live-state column on these pages already
/// uses); plain text otherwise — a rate limit or a failure is a fact about a provider, not a broken
/// screen, so it does not spend the one red the design system reserves for a down service. Nothing
/// while a test has never run — the caller decides what "in flight" looks like, since that is a
/// state of the *button*, not of a result that does not exist yet.
pub(crate) fn test_verdict_view(result: Option<TestResultDto>) -> impl IntoView {
    let Some(result) = result else {
        return ().into_any();
    };
    if result.verdict == "ok" {
        return view! {
            <span class="adi-status" data-state="online">
                <span class="adi-status__led"></span>
                <span>{result.message}</span>
            </span>
        }
        .into_any();
    }
    view! { <span class="adi-muted">{result.message}</span> }.into_any()
}

/// A two-option segmented toggle bound to a `bool` signal: the left button selects `false`, the
/// right selects `true`, each reflecting the state through `aria-pressed`.
pub(crate) fn segmented(
    aria_label: &'static str,
    signal: RwSignal<bool>,
    left: &'static str,
    right: &'static str,
) -> impl IntoView {
    view! {
        <div class="adi-segmented" role="group" aria-label=aria_label>
            <button class="adi-segmented__option" type="button"
                aria-pressed=move || (!signal.get()).to_string()
                on:click=move |_| signal.set(false)>{left}</button>
            <button class="adi-segmented__option" type="button"
                aria-pressed=move || signal.get().to_string()
                on:click=move |_| signal.set(true)>{right}</button>
        </div>
    }
}

/// A labeled text input bound to a `String` signal — the `adi-field` wrapper the forms repeat.
/// `id` is both the input's `id` and the label's `for`. Optional props toggle the mono/wide input
/// classes, a numeric input mode, a trailing hint line, and extra classes on the field wrapper
/// (e.g. `adi-field--grow`).
#[component]
pub(crate) fn TextField(
    id: &'static str,
    label: &'static str,
    value: RwSignal<String>,
    #[prop(optional)] placeholder: &'static str,
    #[prop(optional)] hint: &'static str,
    #[prop(optional)] mono: bool,
    #[prop(optional)] wide: bool,
    #[prop(optional)] numeric: bool,
    #[prop(optional)] field_class: &'static str,
) -> impl IntoView {
    let mut class = String::from("adi-input");
    if wide {
        class.push_str(" adi-input--wide");
    }
    if mono {
        class.push_str(" adi-mono");
    }
    let mut field = String::from("adi-field");
    if !field_class.is_empty() {
        field.push(' ');
        field.push_str(field_class);
    }
    let inputmode = if numeric { "numeric" } else { "text" };
    view! {
        <div class=field>
            <label class="adi-field__label" for=id>{label}</label>
            <input class=class id=id placeholder=placeholder autocomplete="off" inputmode=inputmode
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev)) />
            {(!hint.is_empty()).then(|| field_hint(hint))}
        </div>
    }
}

/// A field's explanation, rendered as a “?” beside its label that opens the text on hover or
/// keyboard focus. Written after the control in the markup; the field's grid places it next to the
/// title, so it costs the form no vertical space.
pub(crate) fn field_hint(text: impl IntoView + 'static) -> AnyView {
    view! {
        <span class="adi-field__hint" tabindex="0" role="note">
            <span class="adi-field__hint-text">{text}</span>
        </span>
    }
    .into_any()
}

/// A read-only field with a Copy button: selects on focus and copies to the clipboard. `node` lets
/// the button reach the input's live text.
///
/// Two pages hand a long token to a human this way — the Mesh page's id and ticket, and the Fleet
/// page's pairing invite — and in both the field is the fallback for when the intended path (a
/// paste into another machine, a camera) does not work. So it is here rather than on either page:
/// a second copy would be free to drift on the one thing that matters, which is that the text is
/// selected as well as written, for the browser that refuses the clipboard.
pub(crate) fn copy_row(
    node: NodeRef<leptos::html::Input>,
    value: impl Fn() -> String + Send + 'static,
) -> impl IntoView {
    view! {
        <div class="adi-copyrow">
            <input class="adi-input adi-input--wide adi-mono" readonly=true node_ref=node
                prop:value=value
                on:focus=move |ev| select_target(&ev) />
            <button class="adi-btn adi-btn--ghost" type="button"
                on:click=move |_| copy_field(node)>"Copy"</button>
        </div>
    }
}

/// Copy a read-only field's text to the clipboard: select it (a visible affordance and a
/// manual-copy fallback), then write it via `navigator.clipboard` on wasm. Best-effort.
fn copy_field(node: NodeRef<leptos::html::Input>) {
    if let Some(input) = node.get() {
        input.select();
        #[cfg(target_arch = "wasm32")]
        clipboard_write(&input.value());
    }
}

/// One-click clipboard write via `navigator.clipboard.writeText`, as a tiny JS shim — so it
/// needs neither the unstable web-sys Clipboard API nor its cfg flag. wasm target only.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(
    inline_js = "export function adiClipboardWrite(t){ try { if (navigator.clipboard) navigator.clipboard.writeText(t); } catch (e) {} }"
)]
extern "C" {
    #[wasm_bindgen(js_name = adiClipboardWrite)]
    fn clipboard_write(text: &str);
}

/// Select all text of the focused input, so clicking the field readies a manual copy.
fn select_target(ev: &web_sys::FocusEvent) {
    use wasm_bindgen::JsCast as _;

    if let Some(input) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.select();
    }
}

/// Run a mutation that returns fresh state `T`, hand the result to `store`, and flash the error if
/// there was one; toggles `busy` around the request when a form is driving it. The `apply_*`
/// helpers are thin typed wrappers over this, differing only in which page-state signal takes the
/// result.
///
/// Success says nothing: `store` has just replaced the page's data with what the server returned,
/// and that is the report (see [`Flash`]). It also clears whatever the last failure left on
/// screen, so a fixed-and-retried form does not keep wearing the error it just cleared.
pub(crate) fn apply_mutation<T, S, F>(state: State, busy: Option<RwSignal<bool>>, store: S, fut: F)
where
    S: Fn(State, T) + 'static,
    F: std::future::Future<Output = Result<T, String>> + 'static,
{
    apply_mutation_impl(state, busy, store, fut, true);
}

/// [`apply_mutation`], but never naming the panel-wide picker's node in the flash it leaves behind —
/// for the couple of pages (`fleet.rs`, `mesh.rs`) whose every mutation is one of `docs/fleet.md`
/// §14's L3 exemptions (`/api/fleet/*`, `/api/mesh*`) and so never actually reaches whatever the
/// picker points at, however it is set.
pub(crate) fn apply_mutation_local<T, S, F>(
    state: State,
    busy: Option<RwSignal<bool>>,
    store: S,
    fut: F,
) where
    S: Fn(State, T) + 'static,
    F: std::future::Future<Output = Result<T, String>> + 'static,
{
    apply_mutation_impl(state, busy, store, fut, false);
}

/// Shared by [`apply_mutation`] and [`apply_mutation_local`] — the only difference between a bare
/// mutation and one of the picker's exemptions is whether a failure gets the pointed node's name
/// stitched onto it (ADI-MONO-89): the plumbing that sends the request already follows the picker
/// (`fetch::get`/`post`) or doesn't (`fetch::get_local`/`post_local`) on its own, this only decides
/// what the operator is told afterward.
fn apply_mutation_impl<T, S, F>(
    state: State,
    busy: Option<RwSignal<bool>>,
    store: S,
    fut: F,
    name_node: bool,
) where
    S: Fn(State, T) + 'static,
    F: std::future::Future<Output = Result<T, String>> + 'static,
{
    if let Some(b) = busy {
        b.set(true);
    }
    spawn_local(async move {
        match fut.await {
            Ok(v) => {
                store(state, v);
                state.flash.set(None);
            }
            Err(e) => {
                let e = match name_node.then(crate::fetch::panel_source).flatten() {
                    Some(node) => format!("{node}: {e}"),
                    None => e,
                };
                state.flash.set(Some(Flash::err(e)));
            }
        }
        if let Some(b) = busy {
            b.set(false);
        }
    });
}

/// A native confirm dialog, returning `true` only when the user accepts. A browser that has no
/// `confirm` (or denies it) reads as "cancelled", so nothing is destroyed by accident.
///
/// Loud about the panel-wide picker (`docs/fleet.md` §14) before a destructive action runs, not
/// only after (ADI-MONO-89): when it is pointed at a node, that node's name is stitched onto the
/// message, ahead of what the caller wrote, so an operator reading only the first line still sees
/// it before pressing OK. [`confirm_local`] is the same dialog without that — for an action that
/// either never follows the picker at all, or already names its own explicit target.
pub(crate) fn confirm(message: &str) -> bool {
    match crate::fetch::panel_source() {
        Some(node) => confirm_local(&format!(
            "This changes {node}, not this machine — the panel is pointed at it.\n\n{message}"
        )),
        None => confirm_local(message),
    }
}

/// [`confirm`], but never naming the panel-wide picker's node — for `fleet.rs`'s own actions
/// (`/api/fleet/*` is always local, `docs/fleet.md` §14's L3) and for the sessions rail's per-row
/// actions, which already name their own explicit source on the row (§13's K6) and would be
/// misnamed by whatever the picker happens to point at instead.
pub(crate) fn confirm_local(message: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(message).ok())
        .unwrap_or(false)
}

/// A native prompt, returning `Some` only when the user accepts with text. A browser that has no
/// `prompt` (or denies it) reads as "cancelled", so nothing is renamed by accident — the same
/// fail-quiet shape [`confirm`] has for the destructive actions.
pub(crate) fn prompt(message: &str, default: &str) -> Option<String> {
    web_sys::window()?
        .prompt_with_message_and_default(message, default)
        .ok()
        .flatten()
}

/// The "updated Ns ago" label; empty until the first successful load. Generic over the loaded
/// payload — every page has its own state type and only emptiness matters here.
pub(crate) fn updated_text<T>(loaded: RwSignal<Option<T>>, secs_since: RwSignal<u32>) -> String
where
    T: Send + Sync + 'static,
{
    // `with` rather than `get`, so testing emptiness never clones the payload.
    if loaded.with(Option::is_none) {
        return String::new();
    }
    match secs_since.get() {
        0 => "updated just now".to_string(),
        s => format!("updated {s}s ago"),
    }
}

/// Format a service's declared port bindings as `key:port, key:port`, or `—` when it declares none.
pub(crate) fn fmt_ports(ports: &[ServicePort]) -> String {
    if ports.is_empty() {
        "—".to_string()
    } else {
        ports
            .iter()
            .map(|p| format!("{}:{}", p.key, p.port))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// An optional string for a table cell, falling back to an em dash when it's absent.
pub(crate) fn dash(value: Option<String>) -> String {
    value.unwrap_or_else(|| "—".to_string())
}

/// What a conversation is known by: a reader's own name for it, if one has been set (`POST
/// /api/agents/run/rename`), else the task it was opened with. Every surface that names a
/// conversation — the rail, the history tables, Analytics — reads it through here, so a rename
/// takes over the row's title wherever it appears rather than only in the rail it was set from.
pub(crate) fn display_message(r: &AgentRunInfo) -> &str {
    r.title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(&r.message)
}

/// Format a byte count as `N KB` / `N.N MB` / `N.NN GB` — decimal units, so the numbers read the
/// same as Activity Monitor's.
pub(crate) fn fmt_bytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let n = bytes as f64;
    if bytes < 1_000_000 {
        format!("{:.0} KB", n / 1_000.0)
    } else if bytes < 1_000_000_000 {
        format!("{:.1} MB", n / 1_000_000.0)
    } else {
        format!("{:.2} GB", n / 1_000_000_000.0)
    }
}

/// Format a CPU share (percent of one core, so it can exceed 100) for a table cell: whole
/// percents once it's past 10, one decimal below that, where the difference between idle and
/// slightly busy actually matters.
pub(crate) fn fmt_cpu(percent: f32) -> String {
    if percent >= 10.0 {
        format!("{percent:.0}%")
    } else {
        format!("{percent:.1}%")
    }
}

/// The CPU cell for a service row. Separate from [`memory_cell`] rather than one two-`<td>`
/// helper, because a table's column config can hide or reorder either independently.
pub(crate) fn cpu_cell(usage: Option<&ProcessUsage>) -> AnyView {
    usage_cell(usage, |u| fmt_cpu(u.cpu_percent))
}

/// The memory cell for a service row. See [`cpu_cell`].
pub(crate) fn memory_cell(usage: Option<&ProcessUsage>) -> AnyView {
    usage_cell(usage, |u| fmt_bytes(u.memory_bytes))
}

/// One sampled-usage cell: `value` of the sample, or a muted em dash when the service is down
/// (or the host could not sample it). The `title` spells out what the number covers, since it
/// rolls up the listener's whole process tree.
fn usage_cell(usage: Option<&ProcessUsage>, value: impl Fn(&ProcessUsage) -> String) -> AnyView {
    let Some(u) = usage else {
        return view! { <td class="adi-muted">"—"</td> }.into_any();
    };
    let procs = if u.processes == 1 {
        format!("pid {}", u.pid)
    } else {
        format!("pid {} + {} child processes", u.pid, u.processes - 1)
    };
    let title = format!("{procs}, up {}", fmt_uptime(u.uptime_secs));
    view! { <td class="adi-tabnums" title=title>{value(u)}</td> }.into_any()
}

/// The capitalized display label for a task's computed effective status, used with the
/// `adi-tstatus` pill.
pub(crate) fn effective_label_title(effective: &str) -> &'static str {
    match effective {
        "ready" => "Ready",
        "blocked" => "Blocked",
        "done" => "Done",
        "archived" => "Archived",
        _ => "—",
    }
}

/// Flatten a flat task list into depth-annotated tree order: each task is immediately followed by
/// its subtree (children in their incoming order), so a caller renders one row per task and indents
/// by the returned depth. A task whose `parent` isn't in the set is treated as a root, so nothing
/// is ever dropped. Depth is unbounded — the tree may nest arbitrarily deep.
pub(crate) fn task_tree_rows(rows: Vec<TaskRow>) -> Vec<(usize, TaskRow)> {
    use std::collections::{HashMap, HashSet};

    let ids: HashSet<String> = rows.iter().map(|r| r.id.clone()).collect();
    let mut children: HashMap<String, Vec<TaskRow>> = HashMap::new();
    let mut roots: Vec<TaskRow> = Vec::new();
    for r in rows {
        match &r.parent {
            Some(p) if ids.contains(p) => children.entry(p.clone()).or_default().push(r),
            _ => roots.push(r),
        }
    }

    fn walk(
        node: TaskRow,
        depth: usize,
        children: &mut HashMap<String, Vec<TaskRow>>,
        out: &mut Vec<(usize, TaskRow)>,
    ) {
        let id = node.id.clone();
        out.push((depth, node));
        if let Some(kids) = children.remove(&id) {
            for kid in kids {
                walk(kid, depth + 1, children, out);
            }
        }
    }

    let mut out = Vec::new();
    for root in roots {
        walk(root, 0, &mut children, &mut out);
    }
    out
}

/// Format a Unix timestamp (seconds) as a `YYYY-MM-DD` UTC date; `0` renders as `—`. Pure
/// integer arithmetic (Howard Hinnant's `civil_from_days`), so no date crate is pulled into wasm.
pub(crate) fn fmt_date(secs: u64) -> String {
    if secs == 0 {
        return "—".to_string();
    }
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

// ---- the workbench's standing advice (persisted dismissal) --------------------------

/// Where the dismissal of the "ask adi-agent" line is remembered. Per browser rather than per
/// machine: it is a preference about how this person reads the page, not a fact about the stack,
/// and the store has no business holding it.
const ADVICE_KEY: &str = "adi-advice-hidden";

/// Whether the advice line above every workbench page has been hidden on this browser.
pub(crate) fn advice_hidden() -> bool {
    storage()
        .and_then(|s| s.get_item(ADVICE_KEY).ok().flatten())
        .is_some_and(|v| v == "1")
}

/// Hide the advice line, and remember it. Hiding something that comes back on the next page load
/// is not hiding it, so this outlives the tab — `adi-advice-hidden` in `localStorage`, which is
/// also how it is undone.
pub(crate) fn hide_advice(hidden: RwSignal<bool>) {
    hidden.set(true);
    if let Some(s) = storage() {
        let _ = s.set_item(ADVICE_KEY, "1");
    }
}

/// The origin's `localStorage`, when there is one. `None` covers private mode and a disabled
/// origin — and, off wasm, the absence of a browser at all, which is what lets the unit tests
/// build a [`TableState`] without reaching for a `window` that isn't there.
pub(crate) fn storage() -> Option<web_sys::Storage> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()?.local_storage().ok().flatten()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}
