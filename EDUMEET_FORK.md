# eduMEET mediasoup fork

## Maintenance baseline

- Fork: `git@github.com:2nterdigital/mediasoup.git`
- Upstream: `https://github.com/versatica/mediasoup.git`
- Upstream release tag: `rust-0.27.0`
- Annotated tag object: `e3d1689556251ae31e1d8e845ae9206d71aad6da`
- Release commit: `7b896f1743b6f4d9d7b237fc1d642828fa6c5b6c`
- Package: `mediasoup 0.27.0`
- Coupled packages: `mediasoup-sys 0.17.0`, `mediasoup-types 0.4.0`
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

The same change, with the remaining upstream hazards it does not cover, is
described in `TORNADO_FORK.md` on the `dev` line of this fork. Drop this
divergence when upstream stops running notification callbacks under the
`EventHandlers` mutex.

The historical `dev@28a0a98161763ceb035872fa642e94d303ca5dd4` commit is not
merged because it was based on mediasoup 0.25.2. Its resource-usage behavior
was reapplied directly to the official 0.27.0 release instead.

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
git diff --name-status 7b896f1743b6f4d9d7b237fc1d642828fa6c5b6c...HEAD
```
