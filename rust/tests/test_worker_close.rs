#![cfg(feature = "test-worker-close")]

use futures_lite::future;
use mediasoup::router::RouterOptions;
use mediasoup::worker::test_worker::{close_worker, worker_ids};
use mediasoup::worker::WorkerSettings;
use mediasoup::worker_manager::WorkerManager;

#[test]
fn closes_the_selected_native_worker_and_cleans_the_registry() {
    future::block_on(async move {
        let worker_manager = WorkerManager::new();
        let worker = worker_manager
            .create_worker(WorkerSettings::default())
            .await
            .expect("Failed to create worker");
        let other_worker = worker_manager
            .create_worker(WorkerSettings::default())
            .await
            .expect("Failed to create second worker");
        let router = worker
            .create_router(RouterOptions::default())
            .await
            .expect("Failed to create router");

        let mut registered_ids = worker_ids();
        registered_ids.sort_unstable();
        let mut expected_ids = vec![worker.id(), other_worker.id()];
        expected_ids.sort_unstable();
        assert_eq!(registered_ids, expected_ids);

        let (mut worker_close_tx, worker_close_rx) = async_oneshot::oneshot::<()>();
        let _worker_close_handler = router.on_worker_close(move || {
            let _ = worker_close_tx.send(());
        });
        let (mut close_tx, close_rx) = async_oneshot::oneshot::<()>();
        let _close_handler = router.on_close(move || {
            let _ = close_tx.send(());
        });

        assert_eq!(close_worker(worker.id()), Some(worker.id()));
        worker_close_rx
            .await
            .expect("Failed to receive worker_close event");
        close_rx.await.expect("Failed to receive close event");

        assert!(worker.closed());
        assert!(router.closed());
        assert!(!other_worker.closed());
        assert_eq!(worker_ids(), vec![other_worker.id()]);
        assert_eq!(close_worker(worker.id()), None);

        drop(router);
        drop(worker);
        drop(other_worker);
        assert!(worker_ids().is_empty());
    });
}
