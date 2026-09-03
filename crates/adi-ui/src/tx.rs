//! [`TxPanel`] — the open transaction: what is staged, what is still open, and the commit.
//!
//! # Why an insert is a transaction at all
//!
//! Inserting is expensive — embed every new fact, compare it against the base, classify what
//! comes back. Refusing the whole batch because one fact is contentious burns all of that and
//! hands the caller nothing to act on. So an insert stages its facts, surfaces the pairs that
//! need a decision, and stays invisible to the rest of the base until every one of them is
//! settled. The person can drop a single fact rather than losing the other nineteen, and the
//! expensive work survives every decision they make.
//!
//! That is the whole reason the commit is disabled while anything is open — and why it says so
//! in words rather than just greying out. A disabled button with no sentence beside it is a
//! screen telling somebody they are wrong without saying about what.

use leptos::prelude::*;

use crate::{Badge, BadgeTone, Button, ButtonSize, ButtonVariant, merge};

/// The open transaction.
///
/// A section, ruled top and bottom: the pending list is its `children`, so this component
/// knows nothing about pairs and the queue knows nothing about committing. Usually the child
/// is a [`crate::PairQueue`].
///
/// Commit is the one action the screen exists for, so it is the screen's orange.
///
/// ```ignore
/// <TxPanel
///     id="tx_7f3a91"
///     staged=Signal::derive(move || staged.get())
///     pending=Signal::derive(move || open.get())
///     on_commit=commit on_abort=abort
/// >
///     <PairQueue pairs=pending on_rule=resolve/>
/// </TxPanel>
/// ```
#[component]
pub fn TxPanel(
    /// The transaction's id, as the store prints it.
    #[prop(optional, into)]
    id: String,
    /// How many facts are staged under it.
    #[prop(into)]
    staged: Signal<usize>,
    /// How many pairs are still waiting on a decision.
    #[prop(into)]
    pending: Signal<usize>,
    /// Make everything staged visible to the base.
    #[prop(into)]
    on_commit: Callback<()>,
    /// Throw the whole transaction away. With no handler the control is not drawn.
    #[prop(optional, into)]
    on_abort: Option<Callback<()>>,
    /// A commit or an abort is in flight.
    #[prop(optional, into)]
    busy: Signal<bool>,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let blocked = Signal::derive(move || pending.get() > 0);

    view! {
        <section class=merge("flex flex-col text-ink", class)>
            <header class="flex flex-wrap items-center gap-2 border-b border-line pb-2.5">
                {(!id.is_empty()).then(|| view! { <Badge mono=true>{id.clone()}</Badge> })}
                <span class="text-row text-ink">
                    {move || {
                        let (s, p) = (staged.get(), pending.get());
                        format!("{s} staged, {p} to decide")
                    }}
                </span>
                <span class="flex-1"></span>
                {move || blocked.get().then(|| view! {
                    <Badge tone=BadgeTone::Warn>"open"</Badge>
                })}
            </header>

            {children()}

            <footer class="flex flex-wrap items-center gap-2 border-t border-line pt-3">
                <span class="min-w-0 flex-1 text-small text-ink-3">
                    {move || {
                        let p = pending.get();
                        if p == 0 {
                            "Every pair is decided. Committing makes the staged facts visible \
                             to the base."
                                .to_string()
                        } else if p == 1 {
                            "1 pair is still open. Nothing lands until it is decided."
                                .to_string()
                        } else {
                            format!("{p} pairs are still open. Nothing lands until every one \
                                     of them is decided.")
                        }
                    }}
                </span>
                {on_abort.map(|cb| view! {
                    <Button
                        size=ButtonSize::Small
                        variant=ButtonVariant::Danger
                        disabled=busy
                        on:click=move |_| cb.run(())
                    >
                        "Abort"
                    </Button>
                })}
                <Button
                    variant=ButtonVariant::Primary
                    size=ButtonSize::Small
                    disabled=Signal::derive(move || blocked.get() || busy.get())
                    on:click=move |_| on_commit.run(())
                >
                    "Commit"
                </Button>
            </footer>
        </section>
    }
}
