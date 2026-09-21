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

The native worker and generated FlatBuffers are unchanged.

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
- `edumeet-rust-0.27.0` (`71ac8c8ebc15139cd78fd2bbd6550433c9d93bcd`) carries the
  resource-usage API only, does not have the deadlock fix and is no longer
  maintained. The resource-usage commit here is cherry-picked from it.

## Verification

Run from the repository root:

```text
cargo test -p mediasoup --lib worker_resource_usage_maps_every_native_protocol_field
cargo test -p mediasoup --test integration notification_dispatch
cargo test -p mediasoup --lib
cargo check -p mediasoup --lib
cargo clippy -p mediasoup --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
git diff --name-status 618400f080905a840e8bcaab7084f43c6228f150...HEAD
```
