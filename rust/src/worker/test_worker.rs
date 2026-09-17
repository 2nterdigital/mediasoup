//! Feature-gated helpers for exercising native worker failure recovery.

use super::{Inner, WorkerId};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

static WORKERS: Lazy<Mutex<HashMap<WorkerId, Weak<Inner>>>> = Lazy::new(Mutex::default);

#[cfg(test)]
type WorkerIdsAfterUpgradeHook = Arc<dyn Fn() + Send + Sync>;

#[cfg(test)]
static WORKER_IDS_AFTER_UPGRADE_HOOK: Lazy<Mutex<Option<(WorkerId, WorkerIdsAfterUpgradeHook)>>> =
    Lazy::new(Mutex::default);

pub(super) fn register(worker: &Arc<Inner>) {
    WORKERS.lock().insert(worker.id, Arc::downgrade(worker));
}

pub(super) fn unregister(worker_id: WorkerId) {
    WORKERS.lock().remove(&worker_id);
}

/// Returns live worker identifiers in deterministic order.
#[must_use]
pub fn worker_ids() -> Vec<WorkerId> {
    let workers = WORKERS
        .lock()
        .iter()
        .map(|(&worker_id, worker)| (worker_id, worker.clone()))
        .collect::<Vec<_>>();

    let mut stale_workers = Vec::new();
    let mut worker_ids = Vec::new();
    for (worker_id, worker) in workers {
        match worker.upgrade() {
            Some(worker_inner) => {
                #[cfg(test)]
                call_worker_ids_after_upgrade_hook(worker_id);

                if worker_inner.closed.load(Ordering::SeqCst) {
                    stale_workers.push((worker_id, worker));
                } else {
                    worker_ids.push(worker_id);
                }
            }
            None => stale_workers.push((worker_id, worker)),
        }
    }

    if !stale_workers.is_empty() {
        let mut workers = WORKERS.lock();
        for (worker_id, stale_worker) in stale_workers {
            if workers
                .get(&worker_id)
                .is_some_and(|worker| Weak::ptr_eq(worker, &stale_worker))
            {
                workers.remove(&worker_id);
            }
        }
    }

    worker_ids.sort_unstable();
    worker_ids
}

/// Closes the selected native worker through the same path used when a worker is dropped.
///
/// Returns the closed worker identifier, or `None` if it was not registered or was already closed.
pub fn close_worker(worker_id: WorkerId) -> Option<WorkerId> {
    let worker = {
        let mut workers = WORKERS.lock();
        workers.remove(&worker_id)?
    };
    let worker = worker.upgrade()?;
    if worker.closed.load(Ordering::SeqCst) {
        return None;
    }

    worker.close();
    Some(worker_id)
}

#[cfg(test)]
fn call_worker_ids_after_upgrade_hook(worker_id: WorkerId) {
    let hook = WORKER_IDS_AFTER_UPGRADE_HOOK.lock().clone();
    if let Some((hook_worker_id, hook)) =
        hook.filter(|(hook_worker_id, _)| *hook_worker_id == worker_id)
    {
        debug_assert_eq!(hook_worker_id, worker_id);
        hook();
    }
}

#[cfg(test)]
mod tests {
    use super::{worker_ids, WORKER_IDS_AFTER_UPGRADE_HOOK};
    use crate::worker::WorkerSettings;
    use crate::worker_manager::WorkerManager;
    use futures_lite::future;
    use std::sync::{Arc, Barrier};

    struct WorkerIdsHookReset;

    impl Drop for WorkerIdsHookReset {
        fn drop(&mut self) {
            WORKER_IDS_AFTER_UPGRADE_HOOK.lock().take();
        }
    }

    #[test]
    fn worker_ids_does_not_hold_the_registry_lock_when_the_snapshot_arc_is_dropped() {
        future::block_on(async move {
            let worker_manager = WorkerManager::new();
            let worker = worker_manager
                .create_worker(WorkerSettings::default())
                .await
                .expect("Failed to create worker");
            let worker_id = worker.id();
            let snapshot_upgraded = Arc::new(Barrier::new(2));
            let worker_dropped = Arc::new(Barrier::new(2));

            let hook_snapshot_upgraded = Arc::clone(&snapshot_upgraded);
            let hook_worker_dropped = Arc::clone(&worker_dropped);
            WORKER_IDS_AFTER_UPGRADE_HOOK.lock().replace((
                worker_id,
                Arc::new(move || {
                    hook_snapshot_upgraded.wait();
                    hook_worker_dropped.wait();
                }),
            ));
            let _hook_reset = WorkerIdsHookReset;

            let drop_thread = std::thread::spawn(move || {
                snapshot_upgraded.wait();
                drop(worker);
                worker_dropped.wait();
            });

            assert!(worker_ids().contains(&worker_id));
            drop_thread.join().expect("Worker drop thread panicked");
            WORKER_IDS_AFTER_UPGRADE_HOOK.lock().take();

            assert!(!worker_ids().contains(&worker_id));
        });
    }
}
