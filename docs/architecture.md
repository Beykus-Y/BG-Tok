# Architecture direction

BG Tok is a library, not an application or player. BeyGram owns UI, explicit
login interaction, secure storage, and composition. BG Media owns presentation
order, playback, preload policy, RAM and the sole persistent media cache. BG Tok
would own authorized service interaction and supply metadata and requested
media bytes. It must not prefetch bytes independently or create a second disk
cache. There is no direct dependency between the two cores.

A host adapter must eventually negotiate a versioned, service-neutral provider
contract with BG Media: namespaced identifiers, metadata, source expiry and
cache policy, capabilities, bounded range/stream requests, cancellation, typed
errors and actual playback events. Ownership, lifetime, thread and shutdown
rules need agreement before an ABI is stabilized. No secret or signed URL belongs
in logs or public metadata. The host supplies secure storage and network/proxy
configuration. No playback event may be inferred from preloading.

These are design constraints, not implemented interfaces. Network, storage,
FFI and platform bindings are intentionally deferred until the feasibility gate
in [feasibility.md](feasibility.md) is satisfied. In particular, an OAuth token
is not a TikTok app session and a mock cannot demonstrate a real feed.
