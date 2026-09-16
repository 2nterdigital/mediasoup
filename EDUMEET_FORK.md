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

The only intentional source divergence from the release commit is the public
Worker resource-usage API needed for Media Node load placement:

- `rust/src/worker/resource_usage.rs` owns the public value, native
  `WorkerGetResourceUsage` request mapping, and focused mapping test; and
- `rust/src/worker.rs` exposes `WorkerResourceUsage` and
  `Worker::get_resource_usage()` through the existing Worker interface.

The native worker, generated FlatBuffers, Worker lifecycle, Router and
PipeTransport behavior, transports, and resource cleanup are unchanged.

The historical `dev@28a0a98161763ceb035872fa642e94d303ca5dd4` commit is not
merged because it was based on mediasoup 0.25.2. Its resource-usage behavior
was reapplied directly to the official 0.27.0 release instead.

## Verification

Run from the repository root:

```text
cargo test -p mediasoup --lib worker_resource_usage_maps_every_native_protocol_field
cargo test -p mediasoup --lib
cargo check -p mediasoup --lib
cargo clippy -p mediasoup --lib -- -D warnings
cargo fmt --all -- --check
git diff --check
git diff --name-status 7b896f1743b6f4d9d7b237fc1d642828fa6c5b6c...HEAD
```
