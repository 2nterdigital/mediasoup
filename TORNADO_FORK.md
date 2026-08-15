# Tornado mediasoup fork

## Maintenance baseline

- Fork: `https://github.com/2nterdigital/mediasoup`
- Upstream: `https://github.com/versatica/mediasoup`
- Package: `mediasoup 0.25.2`
- Release commit: `67b4a47c831b79d348e7e451a5ea648ba0d3c3fd`
- Coupled packages: `mediasoup-sys 0.15.2`, `mediasoup-types 0.4.0`
- License: ISC

Consumers must pin a full Tornado fork commit SHA together with exact version
`=0.25.2`. A branch name is a maintenance pointer, not a reproducible pin.

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
