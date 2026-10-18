//! One cooperative deadline for preparation, traversal and regular-path verification.
//! An atomic operation can finish after expiry; no new work may begin afterwards.

use crate::clock::Instant;
use ournotes_sim::Error;
use std::time::Duration;

#[derive(Clone, Copy)]
pub(crate) struct SearchBudget {
    deadline: Option<Instant>,
}

impl SearchBudget {
    pub(crate) fn new(start: Instant, limit: Option<Duration>) -> Result<Self, Error> {
        let deadline = limit
            .map(|duration| {
                start.checked_add(duration).ok_or_else(|| Error::Input("time limit exceeds the clock range".into()))
            })
            .transpose()?;
        Ok(Self { deadline })
    }

    pub(crate) fn deadline(self) -> Option<Instant> {
        self.deadline
    }

    pub(crate) fn expired(self) -> bool {
        crate::cancel::requested() || self.deadline.is_some_and(|deadline| now() >= deadline)
    }

    /// `expired` at a clock value the caller has just read with [`now`].
    pub(crate) fn expired_at(self, now: Instant) -> bool {
        crate::cancel::requested() || self.deadline.is_some_and(|deadline| now >= deadline)
    }
}

pub(crate) fn now() -> Instant {
    #[cfg(test)]
    if let Some(now) = test_clock::now() {
        return now;
    }
    Instant::now()
}

// No clock injection exists in release/public APIs. These stage markers only let
// unit tests advance a monotone clock at meaningful preparation/verification work.
#[cfg(test)]
pub(crate) mod test_clock {
    use crate::clock::Instant;
    use std::cell::RefCell;
    use std::time::Duration;

    struct Clock {
        time: Instant,
        stage: &'static str,
        after_hits: usize,
        hits: usize,
    }
    thread_local! {
        static CLOCK: RefCell<Option<Clock>> = const { RefCell::new(None) };
    }
    pub(crate) fn now() -> Option<Instant> {
        CLOCK.with(|clock| clock.borrow().as_ref().map(|clock| clock.time))
    }
    pub(crate) fn stage(name: &'static str) {
        CLOCK.with(|clock| {
            let mut clock = clock.borrow_mut();
            if let Some(clock) = clock.as_mut()
                && clock.stage == name
            {
                clock.hits += 1;
                if clock.hits == clock.after_hits {
                    clock.time += Duration::from_secs(1);
                }
            }
        });
    }
    pub(crate) fn with_expiry<T>(stage: &'static str, after_hits: usize, f: impl FnOnce() -> T) -> T {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                CLOCK.with(|clock| *clock.borrow_mut() = None);
            }
        }
        CLOCK.with(|clock| {
            assert!(clock.borrow().is_none());
            *clock.borrow_mut() = Some(Clock { time: Instant::now(), stage, after_hits, hits: 0 });
        });
        let _reset = Reset;
        f()
    }
}
