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

The fork retains the worker resource-usage API approved by tornado-media
Decision 0005:

- `rust/src/messages.rs` maps the native `WorkerGetResourceUsage` response;
- `rust/src/worker.rs` exposes `WorkerResourceUsage` and
  `Worker::get_resource_usage()`; and
- the focused unit test maps all sixteen existing FlatBuffers fields.

The native worker, generated FlatBuffers, and the resource-usage request are
unchanged. The fork only exposes the request already implemented upstream.

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
cargo test -p mediasoup --lib
cargo clippy -p mediasoup --lib -- -D warnings
cargo fmt --all -- --check
git diff --check
```
