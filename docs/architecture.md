# Architecture

BG Tok is a library, not an application or player. BeyGram owns UI, explicit
login interaction, secure storage, and composition. BG Media owns presentation
order, playback, preload policy, RAM and the sole persistent media cache. There
is no direct dependency between the two cores. No bytes are fetched unless the
host requests them and no playback event is inferred from fetching.

## Current Rust interfaces

* `ContentProvider` describes paginated metadata, opaque source handles,
  capabilities, byte streams, actions, explicit playback events and logout.
  The host composes a provider for each authorized session. Source IDs and
  metadata must never contain credentials or signed URLs. A provider keeps
  sensitive URL mappings internally. These Rust interfaces are provisional,
  not a stable or agreed C ABI.
* `Feed` serializes page loads, preserves order, deduplicates IDs, detects cursor
  cycles and retains a bounded number of metadata items. Failed pages do not
  advance the cursor. Index lookup is not a presentation queue. Closing a feed
  or cancelling its parent session aborts pending work; an individual call can
  also be cancelled. A host should discard old snapshots on account switch.
* `Accounts` delegates storage to a host `SecureStore`. Session secrets are
  zeroed on drop; hosts must also protect any copies made by their storage or
  providers. Switching accounts cancels the previous session. Logout cancels
  work and deletes local credentials; the host must invoke provider-side
  revocation separately where available. In-memory test storage is not secure.
* `MediaDelivery` opens only requested streams and refreshes an expired source
  once before streaming. A stream is pull-driven, cancels on session or request
  cancellation, and is dropped by the consumer. Errors after the first byte are
  returned without automatic replay. The host controls caching and backpressure.
* `HttpTransport` provides HTTP(S), explicit HTTP(S)/SOCKS proxy configuration,
  bounded concurrency, per-attempt deadlines, cancellation and bounded retry
  of GET before body delivery. Other methods are never automatically retried.
  A valid range response is required for range requests. Response chunks are
  transport-sized, not a fixed maximum; consumers must impose any required
  byte budget and must not collect arbitrary streams in memory. HTTP response
  headers and request data can be sensitive and must not be logged.

`bg-tok-mock` is a separate package and contains only synthetic data. Nothing
in the core depends on it. No implementation performs TikTok requests.

## Integration still needed

Agree with BG Media on a versioned neutral ABI including ownership, lifetimes,
threading, shutdown, cancellation, cache policy and playback event semantics;
then implement and test bindings for target platforms. Provider implementations
must use permitted service APIs, isolate accounts and handle revocation. The
Rust interface, local mock, and unit tests do not demonstrate real TikTok access,
playback rights or recommendation feedback. See [feasibility](feasibility.md).
