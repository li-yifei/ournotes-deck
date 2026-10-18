//! Cooperative cancellation for hosts that run a search in-process (the desktop FFI).
//!
//! The search reads its time budget at every point where it may stop. A cancellation requested for the current
//! thread makes those reads report the budget as spent, so the search ends as if its time limit expired.

use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// A cancellation flag shared between a host and the search it started.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Cancellation>> = const { RefCell::new(None) };
}

/// Runs `f` with `cancel` observed by searches on this thread.
pub fn with_cancellation<T>(cancel: Cancellation, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Cancellation>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|c| *c.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CURRENT.with(|c| c.borrow_mut().replace(cancel)));
    f()
}

/// Whether the host cancelled the search on this thread.
pub(crate) fn requested() -> bool {
    CURRENT.with(|c| c.borrow().as_ref().is_some_and(Cancellation::is_cancelled))
}
