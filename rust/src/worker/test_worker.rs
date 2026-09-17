//! Feature-gated helpers for exercising native worker failure recovery.

use super::{Inner, WorkerId};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

static WORKERS: Lazy<Mutex<HashMap<WorkerId, Weak<Inner>>>> = Lazy::new(Mutex::default);

pub(super) fn register(worker: &Arc<Inner>) {
    WORKERS.lock().insert(worker.id, Arc::downgrade(worker));
}

pub(super) fn unregister(worker_id: WorkerId) {
    WORKERS.lock().remove(&worker_id);
}

/// Returns live worker identifiers in deterministic order.
#[must_use]
pub fn worker_ids() -> Vec<WorkerId> {
    let mut workers = WORKERS.lock();
    workers.retain(|_, worker| {
        worker
            .upgrade()
            .is_some_and(|worker| !worker.closed.load(Ordering::SeqCst))
    });

    let mut worker_ids = workers.keys().copied().collect::<Vec<_>>();
    worker_ids.sort_unstable();
    worker_ids
}

/// Closes the selected native worker through the same path used when a worker is dropped.
///
/// Returns the closed worker identifier, or `None` if it was not registered or was already closed.
pub fn close_worker(worker_id: WorkerId) -> Option<WorkerId> {
    let worker = {
        let mut workers = WORKERS.lock();
        let worker = workers.remove(&worker_id)?.upgrade()?;
        if worker.closed.load(Ordering::SeqCst) {
            return None;
        }
        worker
    };

    worker.close();
    Some(worker_id)
}
