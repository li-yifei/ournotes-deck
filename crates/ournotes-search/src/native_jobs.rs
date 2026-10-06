//! Bounded native batches. The caller participates; N states use at most N threads.
//! Mutable simulation caches have one owner for the entire batch. Results are
//! restored to input order before the coordinator updates any proof state.
use crossbeam_deque::{Injector, Steal};
use ournotes_sim::Error;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) fn map<S: Send, T: Sync, R: Send>(
    states: &mut [S],
    tasks: &[T],
    cancelled: impl Fn() -> bool + Sync,
    run: impl Fn(&mut S, &T) -> Result<R, Error> + Sync,
) -> Result<Option<Vec<R>>, Error> {
    if states.is_empty() {
        return Err(Error::Domain("native batch needs a worker".into()));
    }
    let queue = Injector::new();
    for index in 0..tasks.len() {
        queue.push(index);
    }
    let failed = AtomicBool::new(false);
    let work = |state: &mut S| {
        let mut results = Vec::new();
        while !cancelled() && !failed.load(Ordering::Relaxed) {
            let index = match queue.steal() {
                Steal::Success(index) => index,
                Steal::Retry => continue,
                Steal::Empty => break,
            };
            let result = run(state, &tasks[index]);
            if result.is_err() {
                failed.store(true, Ordering::Relaxed);
            }
            results.push((index, result));
        }
        results
    };
    let mut results = std::thread::scope(|scope| {
        let (first, rest) = states.split_first_mut().expect("nonempty workers");
        let handles: Vec<_> = rest.iter_mut().map(|state| scope.spawn(|| work(state))).collect();
        let mut results = work(first);
        let mut panicked = false;
        for handle in handles {
            match handle.join() {
                Ok(batch) => results.extend(batch),
                Err(_) => panicked = true,
            }
        }
        if panicked { Err(Error::Domain("native simulation worker panicked".into())) } else { Ok(results) }
    })?;
    results.sort_by_key(|(index, _)| *index);
    let results = results.into_iter().map(|(_, result)| result).collect::<Result<Vec<_>, _>>()?;
    Ok((results.len() == tasks.len() && !cancelled()).then_some(results))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn bounded_batch_preserves_order_and_visits_once() {
        let calls = AtomicUsize::new(0);
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let tasks: Vec<_> = (0..300).collect();
        let result = map(
            &mut [0; 3],
            &tasks,
            || false,
            |count, &value| {
                let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                calls.fetch_add(1, Ordering::Relaxed);
                *count += 1;
                std::thread::yield_now();
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(value * 2)
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(result, tasks.iter().map(|v| v * 2).collect::<Vec<_>>());
        assert_eq!(calls.load(Ordering::Relaxed), 300);
        assert!(peak.load(Ordering::Relaxed) <= 3);
    }

    #[test]
    fn cancellation_and_errors_do_not_publish_partial_batches() {
        assert!(map(&mut [(); 2], &[1, 2], || true, |_, _| Ok(1)).unwrap().is_none());
        let stop = AtomicBool::new(false);
        assert!(
            map(
                &mut [(); 2],
                &[1, 2, 3],
                || stop.load(Ordering::Relaxed),
                |_, _| {
                    stop.store(true, Ordering::Relaxed);
                    Ok(1)
                }
            )
            .unwrap()
            .is_none()
        );
        assert!(
            map(&mut [(); 2], &[1, 2], || false, |_, _| -> Result<(), Error> { Err(Error::Domain("test".into())) })
                .is_err()
        );
    }
}
