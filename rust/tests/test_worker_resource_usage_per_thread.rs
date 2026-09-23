//! Regression test for ech0-media issue #130: `Worker::FillBufferResourceUsage`
//! (`worker/src/Worker.cpp`) used to call `uv_getrusage()`, which reports whole-*process* usage
//! (`getrusage(RUSAGE_SELF, ...)` on POSIX). Every worker of the same process therefore reported
//! the identical `ru_utime`/`ru_stime`, no matter which worker's own thread was actually busy.
//!
//! This creates two workers and burns CPU on a third, unrelated `std::thread` - never a mediasoup
//! worker thread, never the `async-executor` thread this test itself runs on. Before the fix
//! (`uv_getrusage_thread()`, libuv >= 1.50), each worker's `ru_utime + ru_stime` included that
//! burn (the whole process's accumulated CPU time never resets, so it shows up in *every*
//! `uv_getrusage()` call made afterwards, from any thread). After the fix, an idle worker's own
//! thread never ran that loop, so its reading stays far below the burn - the assertion below fails
//! against the unfixed native worker and passes with it.

use std::thread;
use std::time::{Duration, Instant};

use futures_lite::future;
use mediasoup::worker::WorkerLogLevel;
use mediasoup::worker::WorkerSettings;
use mediasoup::worker_manager::WorkerManager;

/// Wall-clock time the extra thread spends pegging one core. Long enough that even a loaded CI
/// host cannot mistake scheduling noise for it; the assertion threshold below only needs to be a
/// fraction of it for the test to have a real margin either way.
const BURN_DURATION: Duration = Duration::from_millis(2_500);

/// Upper bound on one worker's own reported CPU time (`ru_utime + ru_stime`, converted from the
/// native response's milliseconds) after the burn. A worker that merely started up and answered
/// two `WORKER_GET_RESOURCE_USAGE` requests spends at most a few milliseconds of real CPU; this
/// leaves a wide margin above that and a wide margin below `BURN_DURATION`, so the assertion is
/// unambiguous in both directions.
const WORKER_CPU_CEILING: Duration = Duration::from_millis(1_000);

fn worker_settings() -> WorkerSettings {
    let mut settings = WorkerSettings::default();
    settings.log_level = WorkerLogLevel::Warn;
    settings
}

#[test]
fn worker_resource_usage_does_not_leak_a_non_worker_threads_cpu_burn() {
    future::block_on(async move {
        let worker_manager = WorkerManager::new();
        let worker_a = worker_manager
            .create_worker(worker_settings())
            .await
            .expect("failed to create the first worker");
        let worker_b = worker_manager
            .create_worker(worker_settings())
            .await
            .expect("failed to create the second worker");

        // A plain OS thread, never a mediasoup worker's own thread and never the executor thread
        // this async block runs on - exactly the "process-wide, not per-worker" hazard issue #130
        // describes: a thread with no worker of its own burning CPU that only a process-wide
        // `uv_getrusage()` could possibly see.
        let burn = thread::spawn(|| {
            let start = Instant::now();
            let mut sink: u64 = 0;
            while start.elapsed() < BURN_DURATION {
                // Real, unoptimizable-away CPU work - not a spin-sleep - so this thread actually
                // consumes `BURN_DURATION` of CPU time, not merely wall-clock time.
                sink = sink.wrapping_add(1);
                std::hint::black_box(&mut sink);
            }
        });
        burn.join().expect("the burn thread does not panic");

        // The process's accumulated CPU time never resets, so the burn is visible to a
        // process-wide read made at any point afterward - no timing race with the sample here.
        let usage_a = worker_a
            .get_resource_usage()
            .await
            .expect("worker A answers its resource-usage probe");
        let usage_b = worker_b
            .get_resource_usage()
            .await
            .expect("worker B answers its resource-usage probe");

        let worker_a_cpu = Duration::from_millis(usage_a.ru_utime + usage_a.ru_stime);
        let worker_b_cpu = Duration::from_millis(usage_b.ru_utime + usage_b.ru_stime);

        assert!(
            worker_a_cpu < WORKER_CPU_CEILING,
            "worker A reported {worker_a_cpu:?} of its own CPU time after a {BURN_DURATION:?} \
             burn on an unrelated thread - it leaked whole-process usage (the pre-fix bug)"
        );
        assert!(
            worker_b_cpu < WORKER_CPU_CEILING,
            "worker B reported {worker_b_cpu:?} of its own CPU time after a {BURN_DURATION:?} \
             burn on an unrelated thread - it leaked whole-process usage (the pre-fix bug)"
        );
    });
}
