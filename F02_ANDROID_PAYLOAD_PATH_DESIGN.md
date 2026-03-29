# F-02: Android Payload Path Contract Clarification

> **Status**: Design Complete  
> **Priority**: P0 (Blocks Gate 2 `platform-android` and Gate 3 `texture-adapter`)  
> **Owner**: platform-android-specialist + engine-integration-specialist  
> **Target**: `irondash/texture/src/platform/android/mod.rs`

---

## 1. Problem Statement

### 1.1 Current Behavior (Pre-F-02)

The Android `PlatformTexture<BoxedPixelData>` implementation in `irondash/texture` exhibits the following behavior:

```rust
// In PlatformTexture::mark_frame_available():
pub fn mark_frame_available(&self) -> Result<()> {
    if let Some(provider) = self.pixel_data_provider.as_ref() {
        let payload = provider.get_payload();  // ← Called HERE
        let payload = payload.get();
        // ... lock ANativeWindow, copy pixels, unlock ...
    }
}
```

**Key observation**: On Android, `get_payload()` is invoked during `mark_frame_available()` on the **caller thread** (typically Platform Thread when called from `engine-texture-registry`).

### 1.2 Darwin Comparison

```rust
// In Darwin PlatformTexture::mark_frame_available():
pub fn mark_frame_available(&self) -> Result<()> {
    self.update_requested.store(true, Ordering::Release);
    // ... call textureFrameAvailable on Flutter side ...
    // Payload is fetched LATER via copyPixelBuffer on Raster Thread
}

// In IrondashTexture::copyPixelBuffer():
fn copy_pixel_buffer(&self) -> CVPixelBufferRef {
    do_copy_pixel_buffer(&self.ivars().payload_provider)  // ← Called HERE (Raster)
}
```

**Key observation**: On Darwin, `get_payload()` is invoked via `copyPixelBuffer` on the **Raster Thread**.

### 1.3 Thread Contract Divergence

| Platform | `mark_frame_available()` thread | `get_payload()` thread |
| :------- | :------------------------------ | :--------------------- |
| Android  | Platform Thread                 | **Platform Thread**    |
| Darwin   | Platform Thread                 | **Raster Thread**      |

This divergence violates the design assumption in `ARCHITECTURE.md §6` that `get_payload()` is always a Raster Thread operation.

---

## 2. Root Cause Analysis

### 2.1 Architectural Difference

The root cause is the **push vs pull** model:

- **Android (Push)**: `mark_frame_available()` pushes pixel data into `ANativeWindow` immediately
- **Darwin (Pull)**: `mark_frame_available()` signals Flutter to pull data later via `copyPixelBuffer`

### 2.2 Implications for `code-base`

If left unaddressed, this divergence forces `texture-adapter` to:

1. Maintain platform-specific thread assumptions
2. Risk calling `get_payload()` on Platform Thread for Android (violating the unified contract)
3. Or incorrectly assume Raster Thread for both (causing Android to fail)

---

## 3. Design Decision

### 3.1 Option Analysis

| Option | Description | Pros | Cons |
| :----- | :---------- | :--- | :--- |
| A | Normalize Android to Darwin pull model | Unified contract; matches design docs | Requires significant fork changes; may break zero-copy goals |
| B | Accept divergence; document in `texture-adapter` | Minimal fork changes; preserves current behavior | Forces platform-specific logic in higher-level crates |
| C | Add substrate hook to expose thread contract explicitly | Clear contract; allows future normalization | Moderate fork changes; adds complexity |

### 3.2 Selected Approach: **Option C with Path to Option A**

**Rationale**: 
- Immediate clarity for `code-base` implementers
- Preserves option to normalize later (Option A) without breaking changes
- Minimal disruption to existing fork behavior

---

## 4. Substrate Modification Specification

### 4.1 New Trait: `PayloadPathContract`

Add to `irondash/texture/src/lib.rs`:

```rust
/// Describes the payload fetch timing contract for a platform.
/// 
/// This trait allows higher-level code to reason about when `get_payload()`
/// will be called relative to `mark_frame_available()`.
pub trait PayloadPathContract {
    /// Returns when get_payload() is called relative to mark_frame_available().
    /// 
    /// - Push: get_payload() is called DURING mark_frame_available()
    /// - Pull: get_payload() is called AFTER mark_frame_available() (e.g., during raster)
    fn payload_timing() -> PayloadTiming;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PayloadTiming {
    /// Payload fetched during mark_frame_available() (Android current behavior)
    PushDuringMarkAvailable,
    /// Payload fetched later via callback (Darwin current behavior)
    PullDuringRaster,
}
```

### 4.2 Platform Implementations

**Android** (`irondash/texture/src/platform/android/mod.rs`):

```rust
impl PayloadPathContract for BoxedPixelData {
    fn payload_timing() -> PayloadTiming {
        PayloadTiming::PushDuringMarkAvailable
    }
}

impl PayloadPathContract for NativeWindow {
    fn payload_timing() -> PayloadTiming {
        // NativeWindow doesn't use get_payload(); direct surface access
        PayloadTiming::NotApplicable
    }
}
```

**Darwin** (`irondash/texture/src/platform/darwin/mod.rs`):

```rust
impl PayloadPathContract for BoxedIOSurface {
    fn payload_timing() -> PayloadTiming {
        PayloadTiming::PullDuringRaster
    }
}

impl PayloadPathContract for BoxedPixelData {
    fn payload_timing() -> PayloadTiming {
        // Even PixelData on Darwin goes through SurfaceAdapter → copyPixelBuffer
        PayloadTiming::PullDuringRaster
    }
}
```

### 4.3 Optional: Future Normalization Hook

Add optional trait for platforms that want to normalize to pull model:

```rust
/// Optional trait for platforms that support deferred payload fetch.
/// Android could implement this by deferring ANativeWindow_lock until
/// a later explicit flush call.
pub trait DeferredPayloadFlush: Sized {
    /// Explicitly flush pending payload to the surface.
    /// If not called, flush happens automatically at drop or next mark_frame_available.
    fn flush_payload(&self) -> Result<()>;
}
```

---

## 5. Integration Contract for `code-base`

### 5.1 `texture-adapter` Usage

```rust
// In texture-adapter/src/shared_texture_adapter.rs:
use irondash_texture::{PayloadPathContract, PayloadTiming, BoxedPixelData};

pub struct SharedTextureAdapter {
    // ...
}

impl SharedTextureAdapter {
    pub fn bind(source: Arc<SharedSource>, engine: EngineHandle) -> Result<Self> {
        // Query the payload timing contract
        let timing = <BoxedPixelData as PayloadPathContract>::payload_timing();
        
        match timing {
            PayloadTiming::PushDuringMarkAvailable => {
                // Android path: get_payload() will be called on Platform Thread
                // during mark_frame_available(). Ensure SharedSource is ready.
                self.prepare_for_push_payload()?;
            }
            PayloadTiming::PullDuringRaster => {
                // Darwin path: get_payload() will be called on Raster Thread
                // via copyPixelBuffer. Ensure payload is lightweight.
                self.prepare_for_pull_payload()?;
            }
            _ => {}
        }
        
        Ok(self)
    }
    
    /// Prepare for Android-style push payload (Platform Thread)
    fn prepare_for_push_payload(&self) -> Result<()> {
        // Ensure platform_handle is valid on Platform Thread
        // No additional synchronization needed
    }
    
    /// Prepare for Darwin-style pull payload (Raster Thread)
    fn prepare_for_pull_payload(&self) -> Result<()> {
        // Ensure payload construction is lightweight
        // May need Arc clone or cached payload
    }
}
```

### 5.2 Thread Assertion Update

Update `thread-dispatcher` to expose platform-specific thread contracts:

```rust
// In thread-dispatcher/src/lib.rs:
pub enum PayloadFetchThread {
    Platform,  // Android: during mark_frame_available
    Raster,    // Darwin: during copyPixelBuffer
}

pub fn payload_fetch_thread_for_platform() -> PayloadFetchThread {
    #[cfg(target_os = "android")]
    return PayloadFetchThread::Platform;
    
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    return PayloadFetchThread::Raster;
}
```

---

## 6. Acceptance Criteria

### 6.1 Fork Changes

- [ ] `PayloadPathContract` trait added to `irondash/texture/src/lib.rs`
- [ ] `PayloadTiming` enum added
- [ ] Android `BoxedPixelData` implements `PushDuringMarkAvailable`
- [ ] Darwin `BoxedIOSurface` implements `PullDuringRaster`
- [ ] Darwin `BoxedPixelData` implements `PullDuringRaster`
- [ ] Documentation updated in `irondash/texture/README.md`

### 6.2 `code-base` Integration

- [ ] `texture-adapter` queries `PayloadTiming` during bind
- [ ] Platform-specific preparation paths implemented
- [ ] Thread assertions updated to reflect actual behavior
- [ ] Tests verify correct thread for payload fetch per platform

---

## 7. Future Work (Post-Gate 3)

### 7.1 Option A: Full Normalization

If zero-copy Android path (F-04) enables deferred payload:

```rust
// Future Android implementation with AHardwareBuffer:
impl DeferredPayloadFlush for BoxedAHardwareBuffer {
    fn flush_payload(&self) -> Result<()> {
        // Lock ANativeWindow, copy from AHB, unlock
        // This is the actual "push" operation
    }
}

// Usage:
// 1. mark_frame_available() sets flag only
// 2. Explicit flush_payload() called when ready (Platform Thread)
// 3. Behavior now matches Darwin pull model
```

### 7.2 Documentation Updates

After F-02 lands, update:
- `ARCHITECTURE.md §6`: Clarify Android vs Darwin thread divergence
- `texture-adapter/DESIGN.md`: Document platform-specific payload paths
- `code-base/progress.md`: Mark F-02 as complete

---

## 8. Risk Assessment

| Risk | Probability | Impact | Mitigation |
| :--- | :--------- | :----- | :--------- |
| Breaking existing Android behavior | Low | High | Preserve current behavior; only add query API |
| Confusion from dual-path contract | Medium | Medium | Clear documentation; examples in `texture-adapter` |
| Delayed zero-copy path (F-04) | Medium | Low | F-02 is independent; zero-copy can land later |

---

## 9. Implementation Plan

### Phase 1: Contract Exposure (Immediate)
1. Add `PayloadPathContract` trait to `irondash/texture`
2. Implement for all current payload types
3. Update `code-base` `texture-adapter` to query contract

### Phase 2: Documentation (Gate 3)
1. Update `ARCHITECTURE.md` with thread divergence notes
2. Add examples to `texture-adapter/DESIGN.md`
3. Create integration tests per platform

### Phase 3: Optional Normalization (Post-Gate 3)
1. Implement `DeferredPayloadFlush` if F-04 enables it
2. Migrate Android to pull model
3. Deprecate `PushDuringMarkAvailable`

---

**F-02 Design Complete**. Ready for implementation.
