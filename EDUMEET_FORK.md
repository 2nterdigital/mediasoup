# eduMEET mediasoup fork

## Maintenance baseline

- Fork: `git@github.com:2nterdigital/mediasoup.git`
- Upstream: `https://github.com/versatica/mediasoup.git`
- Upstream release tag: `rust-0.28.1`
- Annotated tag object: `2f15d8f70ba7f76ba93526140d74fae76a0445a6`
- Release commit: `618400f080905a840e8bcaab7084f43c6228f150`
- Package: `mediasoup 0.28.1`
- Coupled packages: `mediasoup-sys 0.18.1`, `mediasoup-types 0.5.0`
- License: ISC

Consumers must pin the fork with a complete commit SHA. A branch name is a
maintenance pointer, not a reproducible dependency.

## Retained eduMEET divergence

There are two intentional source divergences from the release commit.

### Worker resource usage

The public Worker resource-usage API needed for Media Node load placement:

- `rust/src/worker/resource_usage.rs` owns the public value, native
  `WorkerGetResourceUsage` request mapping, and focused mapping test; and
- `rust/src/worker.rs` exposes `WorkerResourceUsage` and
  `Worker::get_resource_usage()` through the existing Worker interface.

The generated FlatBuffers are unchanged. The native worker itself was unchanged through `p2`; `p3`
(below) is this fork's first native, non-generated divergence from upstream -
`worker/src/Worker.cpp` now reads per-worker-thread CPU time instead of whole-process.

### Notification dispatch deadlocks

Upstream runs every notification callback of a worker while holding the one
`EventHandlers` mutex of that worker's `Channel`. Dropping the subscription of
any entity of the same worker needs that same non-reentrant mutex, so releasing
the last handle of an entity from inside a notification callback parks the
worker thread forever: media, requests and every later entity drop on that
worker stop, and `Worker::on_dead` never fires because the thread does not exit.
The library does this on its own in `AudioLevelObserver` (`Volumes`) and
`ActiveSpeakerObserver` (`DominantSpeaker`), which hand strong `Producer`
handles to the application and drop them before returning, so they are the last
handles whenever the application releases its own while the callback runs; an
application callback that releases an entity does the same. Present upstream
since `rust-0.10.0` and not fixed as of `v3` at
`1d5fbdf085ef644bb7585bce5c52765dd165b78c` (2026-09-21).

- `rust/src/worker/common.rs`: `call_callbacks_with_single_value()` takes a
  snapshot of the target's callbacks and releases the mutex before calling
  them. A callback can therefore still run once after its subscription was
  dropped from another thread. Cost: one `Arc` clone and no allocation per
  notification for a target with one subscriber.
- `rust/src/worker_manager.rs`: once the mutex is released, a running
  notification can be the last owner of the router, the worker and the
  `WorkerManager`. On a worker thread `Inner::drop()` hands its wait for worker
  exits to a helper thread, together with the executor stop signal, instead of
  waiting for the thread it runs on.
- `rust/src/router/data_consumer.rs`: `paused`/`data_producer_paused` are
  locked in one order and released before pause/resume callbacks run, the fix
  upstream applied to `Consumer` in versatica/mediasoup#1605 but not to
  `DataConsumer`.
- `rust/tests/integration/notification_dispatch.rs` reproduces the callback
  cases; all of its tests deadlock without these changes.

Still unsafe, unchanged from upstream:

- Releasing the last `WorkerManager` handle inside a `close`/`*_close` callback
  that runs on the single executor thread of `WorkerManager::new()` parks that
  thread: it waits for worker exits that only a `WorkerClose` task queued on
  itself can cause. Keep a `WorkerManager` handle on an application thread and
  release it there last.
- A callback registered on an entity must capture a `downgrade()`d handle of
  that entity, never a strong clone: dropping its `HandlerId` destroys the
  callback while `event-listener-primitives` holds the bag mutex, and the
  entity's drop then needs the same mutex on the same thread.
- `BufferMessagesGuard::drop()` still replays buffered notifications while
  holding `buffered_notifications_for`. Releasing it first would let the worker
  thread deliver newer notifications of that target before the buffered ones.

Drop this divergence when upstream stops running notification callbacks under
the `EventHandlers` mutex; the tests stay valid either way.

## Lines and releases

- This line, `rust-0.28.1-patched`, is the maintained one. A release is a tag
  `2nt-rust-0.28.1-pN` on it; consumers pin the full commit SHA of a tag. The
  name must not match `rust-X.Y.Z`, which triggers the upstream crate publish
  workflow.
- Released so far: `2nt-rust-0.28.1-p1` (notification dispatch deadlock fix),
  `2nt-rust-0.28.1-p2` (worker registry lock re-entry fix and its close seam,
  cherry-picked from `codex/issue-18-worker-close-seam`, which was based on the
  retired 0.27.0 line), `2nt-rust-0.28.1-p3` (worker resource usage: per-worker-thread CPU via
  `uv_getrusage_thread()` - ech0-media issue #130).
- `edumeet-rust-0.27.0` (`71ac8c8ebc15139cd78fd2bbd6550433c9d93bcd`) carries the
  resource-usage API only, does not have the deadlock fix and is no longer
  maintained. The resource-usage commit here is cherry-picked from it.

## Worker registry lock re-entry (p2)

`test_worker::WORKERS` is a second registry with the same hazard class as the
notification dispatch above: `worker_ids()`/`close_worker()` used to
`Weak::upgrade()` and then drop the resulting strong handle while still holding
the registry lock, so a worker whose drop re-enters that registry deadlocks on
the same non-reentrant mutex on the same thread. Fixed by releasing the guard
before the upgraded handle can drop.

The close seam it comes with is feature-gated: run it with
`cargo test --features test-worker-close --test test_worker_close`. Without the
feature the test file compiles to nothing and reports `0 passed`, which reads
like a pass but runs nothing.

## Worker resource usage: per-worker-thread CPU (p3)

`Worker::FillBufferResourceUsage` (`worker/src/Worker.cpp`) filled every
`WORKER_GET_RESOURCE_USAGE` reply from `uv_getrusage()` - whole-*process* usage
(`getrusage(RUSAGE_SELF, ...)` on POSIX, `GetProcessTimes()`/`GetProcessMemoryInfo()` on Windows).
The Rust binding runs every `Worker` as a thread inside one process (ech0-media: several workers,
one process), so every worker's `ru_utime`/`ru_stime`/`ru_maxrss` carried the identical number,
regardless of which worker actually did the work - ech0-media issue #130, confirmed directly first:
two workers under a real ech0-media node (`cargo test -p ech0-testkit --test ops_test
worker_cpu_series_do_not_leak_a_non_worker_threads_cpu_burn`, against the unfixed `p2` native
worker) both reported `ru_utime=2608ms`/`ru_stime=61ms`/`ru_maxrss=100304` KiB after a 2.5s CPU burn
on an unrelated thread - bit-for-bit identical between the two workers, and matching the burn's own
duration almost exactly.

- `Worker::FillBufferResourceUsage` now reads `uv_getrusage_thread()` (libuv >= 1.50; this fork
  bundles 1.51.0, `worker/subprojects/libuv.wrap`) for `ru_utime`/`ru_stime` and every other field
  except `ru_maxrss`. The request is handled on the calling worker's own libuv loop thread
  (`Worker::HandleRequest()` runs as `Channel::ChannelSocket::Listener`'s callback, itself driven by
  that worker's own `DepLibUV::RunLoop()` - see `Worker::Run()`), so `uv_getrusage_thread()` reports
  exactly that worker's own CPU time - verified against the libuv 1.51.0 source
  (`src/unix/core.c`/`src/win/util.c`): POSIX uses `getrusage(RUSAGE_THREAD, ...)` (Linux) or
  `getrusage(RUSAGE_LWP, ...)` (the BSDs), macOS uses
  `thread_info(mach_thread_self(), THREAD_BASIC_INFO, ...)`, Windows uses `GetThreadTimes()`.
- `ru_maxrss` is deliberately still read from a second, process-wide `uv_getrusage()` call, not
  from `uv_getrusage_thread()`'s own result: a thread's resident set is not a meaningful concept on
  any platform above. Linux/BSD's `RUSAGE_THREAD`/`RUSAGE_LWP` still reports the *process's*
  `ru_maxrss` (every thread of a process shares one address space - one `mm_struct` on Linux);
  macOS and Windows do not fill `ru_maxrss` through the per-thread call at all - libuv zeroes it
  there (`uv_getrusage_thread()`'s own doc: "On macOS and Windows not all fields are set, the
  unsupported fields are filled with zeroes"). Reporting the process-wide number for every worker is
  the honest value; a per-thread `ru_maxrss` would either silently duplicate it (Linux/BSD) or read
  0 (macOS/Windows).
- `uv_getrusage_thread()` falls back to the previous, process-wide `uv_getrusage()` on
  `UV_ENOTSUP` - any platform with neither the macOS path nor `RUSAGE_LWP`/`RUSAGE_THREAD` - instead
  of failing the request; every worker on such a platform keeps reporting the same process-wide
  number it always did, rather than newly breaking `/metrics` there. Every platform this fork
  actually builds for (Linux via `RUSAGE_THREAD`, macOS via `thread_info`, Windows via
  `GetThreadTimes()`) has genuine per-thread CPU time; none is known to hit this fallback.
- `libuv` normalizes `ru_maxrss` to kibibytes on every platform
  (`src/unix/core.c`'s own comment: "Most platforms report ru_maxrss in kilobytes; macOS and
  Solaris are the outliers" - macOS divides its native bytes by 1024, Solaris multiplies its native
  pages). This was already true before `p3`; ech0-media's own `ech0_media_worker_max_rss_bytes`
  metric was exporting that kibibyte value unconverted under a `_bytes` name - fixed on the
  ech0-media side (`crates/ech0-node/src/ops.rs`), not here. This fork's FlatBuffers wire value is
  unchanged (kibibytes, as it always was).
- `rust/tests/test_worker_resource_usage_per_thread.rs`: two workers, a `std::thread` (never a
  worker's own thread) burns 2.5s of real CPU, both workers' own `ru_utime + ru_stime` are asserted
  under 1s afterward. Fails red against the unfixed native worker (both workers ~2.55s - matching
  the ech0-media-level reproduction above almost exactly); passes green with this fix.

## Verification

Run from the repository root:

```text
cargo test -p mediasoup --lib worker_resource_usage_maps_every_native_protocol_field
cargo test -p mediasoup --test integration notification_dispatch
cargo test -p mediasoup --test test_worker_resource_usage_per_thread
cargo test -p mediasoup --lib
cargo check -p mediasoup --lib
cargo clippy -p mediasoup --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
git diff --name-status 618400f080905a840e8bcaab7084f43c6228f150...HEAD
```
