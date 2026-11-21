# BG Tok

Rust provider-independent primitives for BeyGram, with a separate synthetic
provider and CLI. There is no real TikTok provider, login, player or C ABI yet.
The requirements for real service access are in [feasibility](docs/feasibility.md).
See [architecture](docs/architecture.md) for the current interfaces and limits.

## Development

Install stable Rust with rustfmt and clippy:

```sh
cargo check --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo run -p bg-tok-mock
```

The mock package is a separate workspace member, excluded from the default
build and from the core's dependencies. The CLI uses only synthetic data. Tests
use local loopback HTTP servers, never service accounts or the TikTok network.

## BG Media host integration

The optional `bg-tok-range` workspace crate adapts `MediaDelivery` to the
`bg_video_range_v1` callback table defined by BG Media. A host resolves a
specific source, verifies its byte length and immutable revision, constructs
`RangeSource` with a running Tokio I/O runtime, and transfers the callback table
through `into_raw`. Release it exactly once after outstanding calls complete.
BG Media can adopt it with `adoptRangeSource`. Keep playback on a non-Tokio
thread and pass the account session cancellation token to the bridge.

Each callback reads an exact byte range of up to 1 MiB with a caller deadline.
The bridge reuses a refreshed source handle and rejects cache policy changes
within the same handle. It performs no disk writes and emits no playback
events. BG Media owns cache policy and eviction. Hosts must preserve
`NoStore`, invalidate any BG Media cache scope on account logout, and use a
new revision when bytes change.

The repository currently provides synthetic providers and transport building
blocks. The adapter has been tested with the synthetic provider, including a
real MP4 payload in a linked BG Media/FFmpeg smoke run.
