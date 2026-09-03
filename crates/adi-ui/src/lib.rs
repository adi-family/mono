//! `adi-ui` — the adi component library.
//!
//! Leptos components, styled with Tailwind utilities that resolve to the one token file,
//! `design/tokens.css`, and drawn to `design/DESIGN.md`. A host page imports
//! [`styles/ui.css`](./styles/ui.css) and the components bring their own look from there.
//!
//! ```ignore
//! use adi_ui::{Button, ButtonVariant, Panel};
//!
//! view! {
//!     <Panel title="Ports">
//!         <Button variant=ButtonVariant::Primary on:click=reserve>"Reserve"</Button>
//!     </Panel>
//! }
//! ```
//!
//! # The one rule when writing a component
//!
//! **A class name has to appear in the source as a whole string literal.** Tailwind finds
//! classes by scanning this crate's `.rs` files for text that looks like a utility — it
//! never runs the code. So a `match` arm returning `"bg-accent text-on-accent"` is found,
//! and so is a class spelled out in a `view!`, but `format!("bg-{tone}")` is not: the
//! utility is simply never generated and the element renders unstyled. Build the whole
//! literal per branch, as [`ButtonVariant::classes`] does, rather than assembling one from
//! pieces.
//!
//! # Developing
//!
//! `trunk serve` in this directory opens the playground — every component on one page, with
//! hot reload. See the [README](./README.md).

// Leptos components are PascalCase functions by convention, which is not a Rust function name.
#![allow(non_snake_case)]

mod app;
mod ask;
mod attach;
mod badge;
mod button;
mod chat;
mod code;
mod composer;
mod facts;
mod faq;
mod feedback;
mod field;
mod flag;
mod form;
pub mod highlight;
mod icon;
mod input;
mod kbd;
mod mark;
mod markdown;
mod modal;
mod pair;
mod panel;
mod path;
mod rail;
mod session;
mod simulator;
mod staging;
mod table;
mod tokens;
mod toolform;
mod topbar;
mod tree;
mod tx;
mod voice;

pub use app::{AppItem, AppState};
pub use ask::{Ask, AskOption, AskQuestion};
pub use attach::{AttachState, Attached, Attaching, files_of};
pub use badge::{Badge, BadgeTone, Dot, DotTone};
pub use button::{Button, ButtonSize, ButtonVariant};
pub use chat::{Chat, Entry, Image, Queued, Role, ToolCall, ToolState, Turn, by_position};
pub use code::{CodeEditor, CodeFrame, CodeHeight, CodeLog};
pub use composer::Composer;
pub use facts::{Change, Fact, FactCard, FactHistory, FactRow, Moved, NodeKind, Stale, StaleList};
pub use faq::{Faq, Qna};
pub use feedback::{Empty, Flash, FlashKind};
pub use field::Field;
pub use flag::{Flag, FlagList, FlagMark};
pub use form::{Form, Hint};
pub use highlight::{Lang, Tok, highlight};
pub use icon::{Icon, IconSize, Lucide, STROKE, svg as icon_svg};
pub use input::{Input, InputWidth, Select, Textarea};
pub use kbd::Kbd;
pub use mark::{Mark, MarkVariant, SPIN_ORIGIN, lobe_path};
pub use markdown::Markdown;
pub use modal::Modal;
pub use pair::{
    Decided, Pair, PairCard, PairQueue, PairSide, Relation, Ruling, Truncated, Verdict,
};
pub use panel::Panel;
pub use path::{DirEntry, PathPicker, PathRoot, dir_of, leaf_of, trim_dir};
pub use rail::{Rail, RailCard, RailGroup};
pub use session::{SessionItem, SessionState};
pub use simulator::{Simulator, ToolDecl};
pub use staging::{Block, Stop, StopLine, TurnBlocks};
pub use table::{Column, EmptyRow, Layout, Row, Sort, SortKey, Table, TableState, sort_rows};
pub use tokens::{PromptText, Token, TokenStream};
pub use toolform::{Param, ParamKind, ToolForm};
pub use topbar::{Crumb, Crumbs, TopBar};
pub use tree::{Tree, TreeNode, TreeState};
pub use tx::TxPanel;
pub use voice::{MicButton, MicState};

/// Join a component's own classes with whatever the call site passed in `class`.
///
/// Every component here takes an optional `class` prop for the thing a component API can
/// never anticipate — a margin, a grid placement, a width. It lands last in the string,
/// which is also last in the cascade among equal-specificity utilities, so a caller's
/// `class="w-full"` beats the component's own width instead of losing to it at random.
///
/// `extra` arrives owned (a Leptos prop always does), so it is consumed and grown in place
/// rather than borrowed into a fresh allocation.
pub(crate) fn merge(own: &str, mut extra: String) -> String {
    if extra.is_empty() {
        return own.to_string();
    }
    extra.insert(0, ' ');
    extra.insert_str(0, own);
    extra
}
