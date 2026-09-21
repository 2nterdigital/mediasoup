# Tornado mediasoup fork

## Maintenance baseline

- Fork: `https://github.com/2nterdigital/mediasoup`
- Upstream: `https://github.com/versatica/mediasoup`
- Upstream baseline: `v3` at `1d5fbdf085ef644bb7585bce5c52765dd165b78c`
  (merged 2026-09-21), which is release `rust-0.28.1`
  (`618400f080905a840e8bcaab7084f43c6228f150`) plus 11 unreleased upstream
  commits, mostly native worker work on the new bandwidth estimation
- Package: `mediasoup 0.28.1`
- Coupled packages: `mediasoup-sys 0.18.1`, `mediasoup-types 0.5.0`
- License: ISC

Consumers must pin a full fork commit SHA. A branch name is a maintenance
pointer, not a reproducible pin. Commits of this branch made before the sync
are based on `mediasoup 0.25.2` (`67b4a47c831b79d348e7e451a5ea648ba0d3c3fd`).

## Retained Tornado divergence

### Worker resource usage

The fork retains the worker resource-usage API approved by tornado-media
Decision 0005:

- `rust/src/messages.rs` maps the native `WorkerGetResourceUsage` response;
- `rust/src/worker.rs` exposes `WorkerResourceUsage` and
  `Worker::get_resource_usage()`; and
- the focused unit test maps all sixteen existing FlatBuffers fields.

The native worker, generated FlatBuffers, and the resource-usage request are
unchanged. This divergence only exposes the request already implemented
upstream.

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
since `rust-0.10.0` and not fixed as of the baseline above.

- `rust/src/worker/common.rs`: `call_callbacks_with_single_value()` takes a
  snapshot of the target's callbacks and releases the mutex before calling
  them. A callback can therefore still run once after its subscription was
  dropped from another thread. Cost: one `Arc` clone and no allocation per
  notification for a target with one subscriber; measured at about +2 ns for
  the dispatch alone and +4 to +9 ns (3-7 %) on the ~125 ns Rust-side receive
  path of a direct `Consumer` RTP notification (release build, Apple aarch64).
  Avoiding that clone is why upstream dispatches under the mutex
  (versatica/mediasoup#666, #731).
- `rust/src/worker_manager.rs`: once the mutex is released, a running
  notification can be the last owner of the router, the worker and the
  `WorkerManager`, through the handles it reports and through its callback,
  which holds a `Router` in both observers. They are then released on the
  worker thread, where `Inner::drop()` used to wait for that same thread to
  exit. On a worker thread it now hands the wait to a helper thread, together
  with the executor stop signal, which has to outlive the wait so that
  `WorkerClose` is still delivered.
- `rust/src/router/data_consumer.rs`: `paused`/`data_producer_paused` are
  locked in one order and released before pause/resume callbacks run, the fix
  upstream applied to `Consumer` in versatica/mediasoup#1605 but not to
  `DataConsumer`.
- `rust/tests/integration/notification_dispatch.rs` reproduces the callback
  cases (entities released while or from inside a notification callback, pause
  state read from pause/resume callbacks); all of its tests deadlock on the
  upstream baseline. The `paused` -> `data_producer_paused` lock order has no
  test, the inversion window is a few instructions wide: keep the four lock
  sites in `data_consumer.rs` identical to `consumer.rs`.

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

## Upstreamed and omitted historical changes

`DataProducerOptions::new_pipe_transport()` is public upstream beginning with
`mediasoup 0.24.3`, so the old Tornado visibility patch from Decision 0006 is
not carried on this baseline.

The historical `MEDIASOUP_WORKER_THREAD_STACK_BYTES` environment-variable
patch is not part of Decisions 0005/0006 and is not carried. It was not a
public API exposure and no Tornado production configuration consumes it.

## Verification

Run from the repository root:

```text
cargo test -p mediasoup --lib worker_resource_usage_maps_every_fbs_field
cargo test -p mediasoup --test integration notification_dispatch
cargo test -p mediasoup --lib
cargo clippy -p mediasoup --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```
