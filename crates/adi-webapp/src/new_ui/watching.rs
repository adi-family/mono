//! What the screen watches on the live channel, gathered from every part that watches something.
//!
//! The channel takes one list per page ([`live::watch`]), and the screen can show a chat twice —
//! the home screen's chat widget and the chat window. Each part [`join`]s with what it wants and
//! the whole screen's list goes out as one ([`live::watch_all`]), so neither part's list replaces
//! the other's and closing one leaves the other watching.

use std::cell::RefCell;
use std::rc::Rc;

use crate::live::{self, Sub};

type Wants = Rc<dyn Fn() -> Vec<Sub>>;

thread_local! {
    static PARTS: RefCell<(u32, Vec<(u32, Wants)>)> = const { RefCell::new((0, Vec::new())) };
}

/// One part's place in the list; dropping it with [`leave`] takes its watches out.
#[derive(Clone, Copy)]
pub(super) struct Part(u32);

/// Add a part that watches what `wants` returns, asked again on every [`send`].
pub(super) fn join(wants: impl Fn() -> Vec<Sub> + 'static) -> Part {
    PARTS.with(|p| {
        let mut p = p.borrow_mut();
        p.0 += 1;
        let id = p.0;
        p.1.push((id, Rc::new(wants)));
        Part(id)
    })
}

pub(super) fn leave(part: Part) {
    PARTS.with(|p| p.borrow_mut().1.retain(|(id, _)| *id != part.0));
    send();
}

/// Tell the channel what every part wants now. Reads each part's signals, so calling it from an
/// effect re-sends whenever any of them changes.
pub(super) fn send() {
    let wants: Vec<Wants> = PARTS.with(|p| p.borrow().1.iter().map(|(_, w)| w.clone()).collect());
    live::watch_all(wants.iter().map(|w| w()).collect());
}
