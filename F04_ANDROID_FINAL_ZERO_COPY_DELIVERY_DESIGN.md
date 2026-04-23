# F-04 Follow-up: Android Native Image Service Bridge and Final Zero-Copy Delivery

> **Status**: Design Proposed  
> **Priority**: P0 (Blocks final Android zero-copy closure and full Android memory-pressure reduction)  
> **Owner**: platform-android-specialist + engine-integration-specialist + Tech Lead  
> **Target**: `irondash/texture/src/platform/android/mod.rs`, `irondash/texture/src/lib.rs`

---

## 1. Review Outcome

### 1.1 What The Current Fork Actually Proves

The current Android fork already proves a useful seam:

- a real `AHardwareBuffer` can travel from upper layers into the fork
- `engine-texture-registry` can register an engine-local Android texture for that source
- `mark_frame_available()` can trigger delivery into the engine-local texture

That seam is real, but it is still a copy bridge. The current hot path remains:

1. `AHardwareBuffer_lock()` on the source buffer
2. `ANativeWindow_lock()` on the engine-local destination
3. row-by-row CPU copy
4. `AHardwareBuffer_unlock()` and `ANativeWindow_unlockAndPost()`

So the current seam is a valid milestone, not the final Android zero-copy design.

### 1.2 Alignment With The Intended Product Architecture

This follow-up design is valid only if it stays aligned with the broader product model:

1. the Flutter-side cover widget is a client of a unified shared-resource system, not the owner of shared image resources
2. on cache hit, the widget reuses the existing shared-resource path rather than re-downloading or re-decoding per engine
3. on cache miss, the request still returns to the same host-owned shared resource service for fetch, decode, cache, and reuse
4. only when the shared service is unavailable, the format is unsupported, or the complexity is unjustified may the system fall back to ordinary Flutter `Image`

For book-cover workloads, this naturally implies two business strategies:

- high-reuse covers should prefer the shared-resource service and shared texture path
- low-reuse or cold images may go directly to ordinary Flutter `Image` as a deliberate business fallback

This document does not change that product stance. It defines the missing Android substrate inside the shared path.

### 1.3 Why Native Image Service Bridging Alone Is Not Enough

Bridge-layer ownership inversion solves only part of the problem.

If the host-native image service becomes the source of truth but Android still ends in `AHardwareBuffer -> ANativeWindow` CPU copy, then the system reduces duplicate download, decode, cache, and cross-engine duplication, but it still pays avoidable Android final-hop memory and CPU cost.

So Android memory-pressure reduction has two layers:

1. host-native image service owns fetch, decode, cache, and decoded resource truth
2. the fork imports and delivers that host-owned GPU-visible resource without a bridge-owned destination copy surface

State 1 without State 2 is useful, but it is not final Android zero-copy.

### 1.4 Non-Goals

This design does not redesign:

- the FFI command surface in phase one
- `request-orchestrator` idempotency semantics that already exist
- the stable default CPU-copy fallback path
- business policy for deciding which images should bypass the shared path entirely

---

## 2. Design Goals

The design must satisfy all of the following:

1. preserve the existing stable CPU-copy path as explicit fallback and baseline
2. keep the host-native image service as the source of truth for shared image resources
3. prevent upper layers from faking zero-copy above the substrate
4. add a distinct final-path Android import substrate instead of overloading `ANativeWindow` semantics
5. make frame ownership, generation, fences, release, and teardown explicit
6. support the project's multi-engine shared-source model without premature cleanup
7. fail closed: capability miss or import failure must not silently masquerade as final zero-copy
8. bound Platform Thread work so cache-miss bursts do not degrade into a registration storm
9. keep steady-state updates cheap after registration; `mark_frame_available()` on the final path must be notification-only
10. provide enough observability to prove which runtime path is actually active on device

---

## 3. Selected Architecture

### 3.1 Option Analysis

| Option | Description | Result |
| :----- | :---------- | :----- |
| A | Keep today's seam and only optimize the copy path | Rejected. This keeps `AHardwareBuffer -> ANativeWindow` as the terminal hop. |
| B | Let the host-native image service become source of truth, but keep the fork's final hop as CPU-copy bridge | Rejected. This reduces duplicate source-resource work, but still preserves avoidable Android final-hop cost. |
| C | Move the copy behind higher-level crates and still call it zero-copy | Rejected. This violates the rule that zero-copy must not be faked above the substrate. |
| D | Keep the host-native image service as producer, add a consumer-import final path in the fork, and retain today's seam as fallback | Selected. This is the only option that satisfies both bridge-layer and final zero-copy goals. |

### 3.2 Two-Level Routing Model

The architecture must distinguish two different routing decisions.

Business-level routing:

- shared-resource route for reusable, high-value images
- ordinary Flutter `Image` fallback for unsupported or not-worth-sharing images

Delivery routing inside the shared-resource route:

- `CpuCopyBridge`: today's seam and explicit fallback path
- `HardwareBufferImport`: the true final Android zero-copy path

Conflating those two decisions would recreate the confusion this document is meant to resolve.

### 3.3 Architectural Decision

The fork must expose a second Android registration path dedicated to final hardware-buffer delivery.

That path must be separate from today's `Texture<NativeWindow>::new_with_hardware_buffer_source` seam. The seam path remains valid for:

- current smoke coverage
- incremental rollout
- explicit fallback when final-path capability checks fail

The new final path must not expose `ANativeWindow` as the public delivery abstraction.

### 3.4 Phase-One Backend Selection

This document is only implementation-ready if phase one selects a single backend rather than leaving consumer import abstract.

Phase one therefore selects a **fork-owned EGL or GPU consumer-import backend inside `irondash`**.

That means:

- the fork owns creation and destruction of consumer-side import objects in phase one
- the Platform Thread owns the EGL context required for import-object creation and destruction
- fence waiting and generation selection must not block the Platform Thread
- registration-time fallback must trigger if the required EGL, GL, or import extensions are unavailable
- the design must not claim final zero-copy on devices where the selected backend cannot satisfy explicit release and teardown rules

Direct engine-supported hardware-buffer import remains a possible later phase, but it is not the backend this document freezes for the first implementation.

---

## 4. Bridge-Layer Boundary

### 4.1 Source Of Truth Ownership

On the target architecture, Android responsibilities are:

| Layer | Responsibility |
| :---- | :------------- |
| Host-native image service | Download, decode, transformed result selection, primary cache truth, canonical decoded resource ownership, and producer-side frame lease source |
| `resource-manager` | Sole writer of `SourceLifecycleState`; owns publication, eviction decisions, and the rule that source cleanup cannot complete while borrows or outstanding frame leases remain |
| `SharedSource` | Retained handle owner plus deferred cleanup routing; it is not the source-lifecycle authority |
| `request-orchestrator` | Admission policy, queue or reject decisions, priority policy, and feedback handling from registration and import backlog |
| `platform-android` | Adapt host-owned frame leases into `SharedSource` and fork-facing provider contracts |
| `engine-texture-registry` | Sole writer of `TextureLifecycleState`, immutable registration identity, and bounded registration execution queue |
| `irondash` fork | Engine-local texture creation plus final consumer-side import, retirement, and teardown |

The final path must therefore be expressed in terms of host-owned frame leases, not fork-owned decode requests. The protocol single-writer rules remain unchanged: `resource-manager` owns `SourceLifecycleState`, and `engine-texture-registry` owns `TextureLifecycleState`.

### 4.2 What The Fork Must Not Own

For the final path, the fork must not become:

- another downloader or cache owner keyed by resource URL or resource key
- a second decode service for assets the host-native image service already owns
- a place that silently recreates per-engine decoded platform resources in steady state
- a hidden downgrade layer that advertises zero-copy while still performing CPU-copy delivery

### 4.3 Host Integration Contract

The expected host-side behavior is:

1. the cover widget asks the shared-resource service for a shared image resource
2. on hit, the service returns the existing shared resource or its final-path-capable handle
3. on miss, the same service performs fetch, decode, cache insert, and shared-resource publication
4. only after that shared resource exists do the Android adaptation and engine registration paths proceed

The design therefore assumes a host-owned producer and a fork-owned delivery substrate.

### 4.4 Producer Contract Must Be Frozen Before Implementation

Before specialist implementation starts, the producer-side contract must be frozen in one place.

That contract must define at least:

- who allocates, retains, and releases each `AHardwareBuffer`
- whether the same underlying buffer may be leased concurrently to multiple engines
- generation monotonicity scope, which is per shared source in phase one
- `release_token` uniqueness scope, which must be unique per shared source until its matching release is acknowledged
- the semantics of `acquire_latest_frame(after_generation)`, including how `NoNewFrame` is reported
- thread guarantees for acquire, release, fence-FD duplication, and fence-FD closure
- the explicit failure surfaces for producer unavailable, stale source, timeout, and unsupported format

### 4.5 Why This Still Requires Final Zero-Copy

Bridge-layer alignment alone improves total system cost by reducing duplicate source work.

It does not by itself remove the Android-side cost of a bridge-owned destination surface and CPU frame transfer. Final zero-copy is still required if the project wants to claim Android-side memory-pressure reduction end to end.

---

## 5. Fork Public Contract

### 5.1 Delivery Mode

Add an explicit Android delivery mode to the fork surface:

```rust
pub enum AndroidTextureDeliveryMode {
    CpuCopyBridge,
    HardwareBufferImport,
}
```

`CpuCopyBridge` covers today's seam and any explicit fallback that still ends in copy behavior.

`HardwareBufferImport` is reserved for the true final path.

### 5.2 Frame Provider Contract

The final path must not consume a bare retained handle with implicit ownership. It needs an explicit per-frame contract driven by host-owned resources:

```rust
pub struct HardwareBufferFrame {
    pub buffer: AHardwareBufferHandle,
    pub width: i32,
    pub height: i32,
    pub format: AndroidHardwareBufferFormat,
    pub generation: u64,
    pub acquire_fence_fd: Option<std::os::fd::OwnedFd>,
    pub release_token: u64,
}

pub struct HardwareBufferFrameRelease {
    pub release_token: u64,
    pub release_fence_fd: Option<std::os::fd::OwnedFd>,
}

pub enum AcquireFrameOutcome {
  Acquired(HardwareBufferFrame),
  NoNewFrame { latest_generation: u64 },
}

pub trait AHardwareBufferFrameProvider: Send + Sync {
  fn acquire_latest_frame(&self, after_generation: u64) -> Result<AcquireFrameOutcome>;
    fn release_frame(&self, release: HardwareBufferFrameRelease) -> Result<()>;
}
```

Rationale:

- `generation` prevents stale-buffer reuse from being invisible
- `release_token` lets the provider correlate completion with the exact acquired frame and must stay unique until acknowledged
- `NoNewFrame` makes coalescing implementable without forcing a failure path
- phase-one `acquire_latest_frame(after_generation)` must be nonblocking with respect to the Platform Thread
- the same underlying buffer may be leased to multiple engines only if the producer can issue independent release tokens and retirement guarantees per lease
- optional fence FDs define the ownership model even if the first shipping implementation temporarily returns `None`

### 5.3 Final-Path Texture Type

Do not overload the current `Texture<NativeWindow>` semantics.

Add a distinct final-path registration API:

```rust
pub struct ImportedHardwareBufferTexture;

impl Texture<ImportedHardwareBufferTexture> {
    pub fn new_with_hardware_buffer_frame_source(
        engine_handle: i64,
        provider: Arc<dyn AHardwareBufferFrameProvider>,
    ) -> Result<Self>;
}
```

Why a separate type is required:

- the final path should not promise `NativeWindow` access if no `NativeWindow` is part of delivery anymore
- it avoids ambiguous behavior between seam and final-path registrations
- upper layers can distinguish real final-path import from an AHB-backed seam that still flushes

### 5.4 Registry-Visible Status

`engine-texture-registry` must expose two different things separately:

1. an **immutable registration path identity** for the lifetime of a `texture_id`
2. a **mutable runtime status** for the active path

Rules:

- the dedupe and queue key for Android registrations is `(engine_handle, source_id, AndroidTextureDeliveryMode)` in phase one
- a path mismatch must never reuse an existing entry implicitly; it requires explicit teardown and explicit re-registration
- a final-path failure must not be hidden behind reuse of a previously registered seam texture

At minimum, the runtime status contract needs a distinction like:

```rust
pub enum AndroidZeroCopyBridgeStatus {
    FlushOnMarkFrameAvailable,
    ReadyForConsumerImport,
}
```

Interpretation:

- `FlushOnMarkFrameAvailable` means the seam path is active and `mark_frame_available()` still performs bridge work
- `ReadyForConsumerImport` means the final path is active and `mark_frame_available()` is notification-only

### 5.5 Revised `mark_frame_available()` Semantics

For the final path, `mark_frame_available()` must become a notification boundary only.

It may:

- record that a new generation exists
- coalesce multiple notifications into one pending import attempt
- wake or schedule the consumer-side import step

It must not:

- call `AHardwareBuffer_lock()` for CPU reads
- call `ANativeWindow_lock()` for destination writes
- perform memcpy-like frame transfer work
- create per-notification uncontrolled Platform Thread work items

### 5.6 Phase-One Thread And Import Contract

Because phase one selects a fork-owned EGL or GPU consumer-import backend, the thread contract must be explicit.

| Operation | Owner thread | Rule |
| :-------- | :----------- | :--- |
| Final-path registration | Platform Thread | create engine-local texture and consumer import objects without blocking waits |
| `mark_frame_available()` | caller to scheduler handoff | notification only; never performs import inline |
| `acquire_latest_frame(after_generation)` | bounded import scheduler | may poll or fetch producer state, but must not block the Platform Thread |
| acquire-fence wait | bounded import scheduler or nonblocking poll | never unbounded on the Platform Thread |
| consumer import bind or destroy | Platform Thread with valid EGL context | explicit ordering and teardown required |
| `release_frame()` | bounded import scheduler after retirement conditions are known | exactly once per acquired frame |

This thread contract is part of feasibility. If a candidate implementation cannot satisfy it, the runtime must fall back before advertising final-path zero-copy.

---

## 6. Ownership, Lifetime, And Memory Model

### 6.1 Actor Responsibilities

| Actor | Responsibility |
| :---- | :------------- |
| Host-native image service | Owns canonical decoded resource lifetime and producer-side frame lease source |
| `resource-manager` | Owns `SourceLifecycleState`, publication, eviction, and the rule that frame leases count against source cleanup |
| `SharedSource` | Owns retained handle transport plus platform cleaner routing only |
| `request-orchestrator` | Owns admission policy, queue or reject policy, priority, and feedback from registration or import pressure |
| `platform-android` frame provider | Adapts host-owned frames into `HardwareBufferFrame` values and accepts completion back via `release_frame()` |
| irondash final-path texture | Imports the acquired frame into the engine-side consumer path and releases it exactly once |
| `engine-texture-registry` | Owns engine-local registration identity, `TextureLifecycleState`, and bounded registration execution |

### 6.2 Memory-Pressure Model

This design treats Android memory pressure in three states:

1. **Without bridge-layer ownership inversion**: host and Flutter duplicate download, decode, cache, and possibly decoded platform resources
2. **With bridge layer only**: duplicate source work decreases, but Android still pays bridge-owned final-hop copy cost
3. **With bridge layer plus final zero-copy**: the host-native service still owns the canonical resource, and the fork imports that shared GPU-visible buffer without a bridge-owned destination copy surface in steady state

The product goal requires State 3, not merely State 2.

### 6.3 Invariants

The final path must enforce these invariants:

1. every successful `acquire_latest_frame(after_generation)` acquisition has exactly one matching `release_frame()` unless teardown consumes the frame through an explicit teardown release path
2. a provider must not recycle or overwrite a generation until its release is acknowledged
3. generation numbers are monotonic per shared source
4. phase one supports at most one in-flight imported frame per engine-local texture; broader queueing requires separate validation
5. one engine releasing its texture must not invalidate another engine's in-flight imported frame for the same shared source
6. engine destroy must drain or downgrade outstanding frames without leaking native references
7. outstanding frame leases count as source borrows for eviction and cleanup purposes
8. a teardown timeout must end in explicit quarantine or delayed retirement semantics, not silent frame drop

### 6.4 Fence Handling

Fence support is mandatory in the contract even if the first shipping implementation temporarily returns `None`.

Rules:

- `acquire_fence_fd` gates when the consumer may safely import or sample the frame
- `release_fence_fd` communicates when the producer may safely reuse or destroy the buffer
- the provider transfers ownership of duplicated acquire-fence FDs to the consumer, and the consumer must close them after wait or handoff
- the consumer transfers ownership of duplicated release-fence FDs back to the provider with `release_frame()`
- missing fence support must be explicit and capability-gated, not silently assumed correct on all devices
- no-fence mode is legal only on an approved device and backend matrix that proves implicit synchronization is sufficient for phase one
- fence waits must never be performed as unbounded blocking waits on the Platform Thread

---

## 7. Final-Path State Machines

### 7.1 Registration State Machine

The engine-local texture instance must distinguish registration path from frame state.

```text
NotRegistered
  -> RegistrationPending
  -> RegisteredBridge        (explicit CPU-copy seam)
  -> RegisteredImport        (final-path registration succeeded)
  -> Failed                  (registration failed)

RegisteredBridge
  -> Released

RegisteredImport
  -> RunningImportStateMachine
  -> TeardownPending
  -> Released
  -> Failed
```

Rules:

- registration failure before advertisement returns `Failed` and no final-path claim is published
- after `RegisteredImport`, the texture may only leave the final path through explicit failure reporting and explicit re-registration, not silent in-place downgrade
- engine destroy moves the instance to `TeardownPending`, not directly to `Released` if a frame is still in flight

### 7.2 Acquire / Import / Release / Teardown State Machine

The final path needs an explicit per-texture import state machine.

```text
Idle
  -> AcquirePending
AcquirePending
  -> ImportPending(frame)
  -> Idle(no_new_frame)
  -> Failed(acquire)
ImportPending(frame)
  -> ImportedVisible(frame)
  -> ReleasePending(frame, import_failed)
ImportedVisible(frame)
  -> ReleasePending(frame, superseded_or_teardown)
ReleasePending(frame)
  -> Idle
  -> Failed(release)

Any nonterminal state
  -> TeardownPending
TeardownPending
  -> Released
  -> Failed(teardown)
```

Semantics:

- `Idle`: no acquired frame is currently owned by the engine-local texture
- `AcquirePending`: the texture is attempting `acquire_latest_frame(after_generation)` against the provider
- `ImportPending(frame)`: a concrete frame has been acquired and is waiting for fence satisfaction or import completion
- `ImportedVisible(frame)`: the frame is currently the engine-visible frame for this texture
- `ReleasePending(frame)`: the engine-local texture still owes `release_frame()` for the acquired frame
- `TeardownPending`: no new acquisitions are allowed; the implementation drains or explicitly abandons the in-flight frame through the provider contract

### 7.3 Notification Coalescing Rules

`mark_frame_available()` must not create an unbounded queue of imports.

Instead, phase one uses these rules:

1. only one acquire or import operation may be in flight per engine-local texture
2. while in `AcquirePending`, `ImportPending`, or `ReleasePending`, additional notifications only update a `latest_requested_generation` marker
3. when the current frame reaches `Idle`, the implementation calls `acquire_latest_frame(after_generation)` at most once, targeting the newest known generation
4. if the provider returns `NoNewFrame`, the state returns to `Idle` without entering failure
5. intermediate generations may be skipped when newer generations supersede them before import

This coalescing rule is required to avoid per-frame import storms under rapid producer updates.

### 7.4 Teardown Rules

Teardown must behave explicitly:

1. stop accepting new acquisitions
2. if a frame is currently acquired, emit exactly one teardown-driven release back to the provider
3. if fence completion is required, wait only within a bounded teardown policy; on timeout, emit explicit failure telemetry and move the frame into an explicit quarantine or delayed-retirement path
4. after teardown finishes, the texture enters `Released` and no later notification may revive it

This prevents engine destroy from leaking frame references or silently discarding provider-owned state.

### 7.5 Cross-Module State Mapping

The new import substate machine does not replace the canonical protocol writers.

Mapping rules:

- `resource-manager` remains the sole writer of `SourceLifecycleState`
- outstanding final-path frame leases count as `Borrowed { borrow_count > 0 }` for source-lifecycle purposes
- `engine-texture-registry` remains the sole writer of `TextureLifecycleState`
- `RegistrationPending` maps to `TextureLifecycleState::Registering`
- `RegisteredImport` with no visible frame yet maps to `TextureLifecycleState::Registered`
- `ImportedVisible(frame)` maps to `TextureLifecycleState::Available`
- `TeardownPending` maps to `TextureLifecycleState::Unregistering`
- terminal import or teardown failure maps to `TextureLifecycleState::Error`
- the import substate machine is internal final-path runtime state, not a second public lifecycle writer

---

## 8. Capability Gating, Fallback, And Load Shaping

### 8.1 Required Capability Checks

The final path must only be selected when all of the following are true:

1. Android API level is high enough for the chosen hardware-buffer import path
2. the buffer usage bits are compatible with consumer-side GPU sampling or import
3. the pixel format is supported by the chosen import backend
4. the engine-side consumer path reports that the import substrate is available
5. the fork can establish a valid lifetime and fence contract for the selected device or runtime combination
6. the host-native producer can provide the required frame ownership guarantees
7. the phase-one fork-owned EGL or GPU import backend and required extensions are present on the device
8. no-fence mode is permitted only when the approved device matrix says it is legal for this backend

### 8.2 Fallback Rules

Fallback must happen in one of three explicitly separated places:

1. **business-routing fallback**: before shared-path registration begins, upper layers deliberately choose ordinary Flutter `Image` because sharing is unavailable, unsupported, or not worth it
2. **registration-time fallback**: capability miss before final-path registration completes; the caller may choose seam registration instead
3. **reacquire-time fallback**: an already-registered final-path texture surfaces explicit failure, tears down, and upper layers decide whether to re-register as seam or route all the way back to ordinary Flutter `Image`

The system must not silently downgrade inside a texture handle that was already advertised as final-path zero-copy.

### 8.3 Admission Ownership And Platform Thread Burst Control

The critical Android risk is not only decode cost. It is the wave of completed misses returning to Platform Thread for registration and first-frame setup.

That risk must be addressed explicitly.

Rules:

1. `request-orchestrator` owns accept, queue, and reject policy using queue-capacity and backlog signals exported by `engine-texture-registry`
2. `engine-texture-registry` owns only the bounded execution of the Platform registration queue; it must not become a second independent admission-policy owner
3. worker-side load completion must enqueue `RegisterTextureCommand`; it must not directly perform registration work inline
4. there must be no hidden unbounded queue between worker completion and registration execution
5. queue records are keyed by `(engine_handle, source_id, AndroidTextureDeliveryMode)` in phase one
6. duplicate queued requests merge in place; priority may only upgrade the existing queued record, not spawn a second registration for the same key
7. a delivery-path mismatch is a typed conflict that requires explicit teardown and explicit re-registration rather than implicit reuse
8. queued work must be canceled before dispatch if the engine is destroyed, the source unloads or fails, or upper layers choose business-routing fallback
9. execution must apply per-engine fairness plus a bounded Platform Thread budget per turn, such as round-robin or aging with bounded count or bounded time slice
10. post-registration final-path import scheduling must also be bounded and must never perform unbounded fence waits on the Platform Thread
11. final-path `mark_frame_available()` after registration must not enqueue a second registration job; it only updates the notification or import scheduler state machine described above

This is the design answer to the concern that cache-miss bursts can destroy Android experience by flooding Platform Thread with registration and first-frame work.

### 8.4 Admission Signals And Metrics

At minimum, the system should expose:

- registration queue depth
- queue wait time
- number of coalesced registrations
- number of rejected, displaced, or path-conflicted registrations
- final-path vs seam registration counts
- explicit fallback reasons
- attempted path vs actual registered path
- failure phase, queue delay, and teardown-timeout outcomes

Without these signals, the system cannot prove that load shaping is working under real cover-list workloads.

---

## 9. Downstream Integration Contract

### 9.1 `platform-android`

`platform-android` is not the final Android image service. It is the adapter between the host-native image service and the fork.

It must:

- implement `AHardwareBufferFrameProvider`
- adapt host-owned frames or handles rather than reintroducing fork-owned decode-by-resource-key semantics on the final path
- implement the frozen producer contract defined before execution begins
- attach generation and release-token bookkeeping to its AHB or EGL path
- preserve producer ownership rules until `release_frame()` acknowledges completion
- route cleanup to the correct Android thread or cleaner path when import teardown requires it

### 9.2 `engine-texture-registry`

The registry must distinguish seam registration from final-path registration and own only bounded registration execution, not global admission policy.

It must:

- treat registration path identity as immutable for the lifetime of a `texture_id`
- expose `FlushOnMarkFrameAvailable` vs `ReadyForConsumerImport`
- keep `get_or_register()` dedup behavior for same-engine reuse
- export queue depth, capacity, and local backlog signals to `request-orchestrator`
- introduce a bounded Platform registration queue and coalescing policy for execution
- never advertise a final-path texture as successful if it has already downgraded internally
- surface explicit final-path failure so upper layers can choose seam re-registration or higher-level fallback

### 9.3 `texture-adapter`

`texture-adapter` stays out of the final Android zero-copy hot path.

Its Android role remains:

- fallback support
- source metadata exposure when needed
- helper adaptation that does not itself become the GPU import layer

It must not become the place where import logic is emulated or where zero-copy claims are reconstructed above the fork.

### 9.4 `ffi-api` And Observability

No immediate ABI expansion is required for phase one of this design.

However, observability must distinguish at least these runtime outcomes:

1. ordinary CPU-copy path
2. explicit AHB seam path that still flushes on `mark_frame_available()`
3. final-path hardware-buffer import
4. final-path registration queued or delayed by admission control
5. final-path failure that triggered explicit teardown and fallback decision

At minimum, emitted events or metrics must carry enough fields to audit those outcomes:

- `attempted_path`
- `registered_path`
- `source_kind`
- `failure_phase`
- `fallback_action`
- `queue_delay_ms`
- `generation`
- `release_token`
- `teardown_timeout`

Without that separation, device smoke and host validation cannot prove the actual runtime behavior.

---

## 10. Acceptance Criteria

This design is considered implemented only when all items below are true.

### 10.1 Bridge-Layer Alignment

- [ ] final-path registration consumes host-owned frames or buffers rather than reintroducing fork-owned decode-by-resource-key semantics
- [ ] the fork remains delivery or import boundary rather than becoming a second image-service owner
- [ ] ordinary Flutter `Image` remains a deliberate upper-layer fallback rather than an implicit internal downgrade
- [ ] `resource-manager` remains the sole writer of `SourceLifecycleState`, and outstanding frame leases block source cleanup

### 10.2 Fork API And State Machine

- [ ] final-path registration API exists independently from `Texture<NativeWindow>`
- [ ] `AHardwareBufferFrameProvider` and frame-release contract exist
- [ ] the provider contract supports `acquire_latest_frame(after_generation)` and `NoNewFrame`
- [ ] registration path identity is immutable for the lifetime of a `texture_id`
- [ ] acquire, import, release, and teardown states are explicit and testable
- [ ] `mark_frame_available()` on the final path no longer performs CPU copy work

### 10.3 Runtime Proof

- [ ] final-path logs or metrics prove no `AHardwareBuffer_lock()` plus `ANativeWindow_lock()` copy on the hot path
- [ ] seam path and fallback path remain available and behaviorally distinct
- [ ] final-path steady state does not depend on a bridge-owned destination copy surface
- [ ] release and engine-teardown paths do not leak outstanding frame references
- [ ] emitted telemetry proves attempted path, actual path, failure phase, and fallback action

### 10.4 Multi-Engine And Memory Safety

- [ ] two engine-local textures can consume the same shared source without premature source cleanup
- [ ] one engine releasing its texture does not invalidate another engine's in-flight imported frame
- [ ] deferred teardown still results in bounded native resource lifetime
- [ ] final-path steady state does not recreate duplicated decoded platform resources per engine beyond required engine-local registration or import references

### 10.5 Platform Thread Load Shaping

- [ ] `request-orchestrator` owns accept, queue, and reject policy using registry-exported capacity signals
- [ ] completed misses enter a bounded Platform registration queue rather than registering inline
- [ ] there is no hidden unbounded queue between worker completion and registration execution
- [ ] duplicate queued registrations for the same `(engine, source, delivery_mode)` are coalesced
- [ ] queue admission, displacement, rejection, and path conflict are observable
- [ ] already-registered final-path textures do not create registration storms on repeated notifications
- [ ] final-path import scheduling is separately bounded and never performs unbounded fence waits on the Platform Thread

### 10.6 Device Validation

- [ ] focused fork tests cover acquire and release token pairing plus generation monotonicity
- [ ] focused tests cover teardown from each nonterminal import state
- [ ] focused tests cover registration queue coalescing and admission limits
- [ ] focused tests cover mixed-path requests for the same `(engine, source)` and require explicit teardown plus re-registration
- [ ] focused tests cover engine destroy while registration is queued or while a notification is pending but no frame is yet visible
- [ ] focused tests cover multi-engine overlapping generations against the same shared source
- [ ] Android target compile remains green
- [ ] single-host real-device smoke proves the final path visually and via instrumentation
- [ ] multi-engine host validation covers shared source, release, engine destroy, reacquire, and final-path memory behavior

---

## 11. Rollout Sequence

Recommended implementation order:

1. freeze the producer-side contract and phase-one backend selection so specialists build against one authoritative import model
2. formalize the bridge-layer boundary so the host-native image service is the source of truth, `resource-manager` owns source lifecycle truth, and the fork is delivery only
3. map the new final-path terms onto canonical protocol and registry ownership so there is one authoritative name for each path and state
4. add the final-path fork API, immutable registration-path identity, and strengthened frame-provider contract without removing the current seam
5. teach `engine-texture-registry` to distinguish seam vs final-path descriptors while exposing queue-capacity telemetry to `request-orchestrator`
6. add bounded Platform registration execution, coalescing, and separately bounded import scheduling before enabling broader final-path rollout
7. implement the `platform-android` frame provider and cleanup or fence plumbing
8. add focused tests for generation, release-token pairing, teardown, queue shaping, mixed-path conflicts, and fallback clarity
9. run single-host real-device smoke on the final path behind explicit capability gating and telemetry
10. only then expand into multi-engine host validation and memory-behavior proof

This order keeps today's stable baseline intact while addressing the actual missing control points: host-owned resource truth, Android final delivery, and Platform Thread burst control.

---

## 12. Summary

The current project no longer lacks an Android AHB entry seam.

What it still lacked before this rewrite was a complete statement of the final Android delivery substrate:

- the host-native image service is the source of truth
- the Flutter cover widget is a client, not the owner
- cache miss returns to the same shared-resource service rather than per-engine download paths
- ordinary Flutter `Image` is an explicit upper-layer fallback, not an internal silent downgrade
- the fork acts as delivery and import boundary
- final Android delivery no longer depends on `AHardwareBuffer -> ANativeWindow` CPU copy
- acquire, import, release, and teardown are explicit state machines
- Platform Thread registration work is bounded so miss bursts do not collapse responsiveness

That is the minimum design change required before the project can honestly claim final Android zero-copy and full Android-side memory-pressure reduction.