# Irondash Fork Modifications Summary

> **Status**: Complete (Wave 1)
> **Date**: 2026-03-29
> **Author**: Tech Lead + Engineering Team
> **Reference**: `../IRONDASH_FORK_WORKBOOK.md`

---

## Executive Summary

This document summarizes all modifications made to the `irondash` fork to support the Flutter multi-engine shared texture system project.

All changes are designed to be:
- **Upstream-compatible**: Changes can be maintained as a fork diff
- **Project-specific hooks**: Extension points for `code-base` integration
- **Backward-compatible**: Existing APIs preserved with default behavior

---

## Modification Index

| ID | Name | Target Module | Status | Downstream Consumer |
| :-- | :--- | :------------ | :----- | :------------------ |
| F-01 | Registration Error Normalization | `texture/src/error.rs` | ✅ Complete | `engine-texture-registry` |
| F-02 | Android Payload Path Contract | `texture/src/lib.rs`, `platform/android/mod.rs` | ✅ Complete | `texture-adapter` |
| F-03 | iOS SurfaceCache Ownership | `texture/src/lib.rs`, `platform/darwin/mod.rs` | ✅ Complete | `platform-ios`, `shared-source` |
| F-04 | Android Zero-Copy Extension | `texture/src/platform/android/mod.rs` | ✅ Complete | `platform-android`, `texture-adapter` |
| F-05 | Engine Teardown Helper | `texture/src/platform/*/mod.rs` | ✅ Complete | `ffi-api`, `engine-texture-registry` |

---

## F-01: Registration Error Normalization

### Purpose

Distinguish registration failure modes without platform-specific string matching.

### Changes

**File**: `texture/src/error.rs`

```rust
// NEW: RegistrationFailureMode enum
pub enum RegistrationFailureMode {
    InvalidThread,
    InvalidHandle,
    NativeRegistrationFailed,
    DuplicateRegistration,
}

// MODIFIED: Error enum with structured variants
pub enum Error {
    TextureRegistrationFailed {
        mode: RegistrationFailureMode,
        detail: Option<String>,
    },
    TextureOperationFailed {
        detail: Option<String>,
    },
    // ...
}

// NEW: Helper methods
impl Error {
    pub fn invalid_thread() -> Self;
    pub fn invalid_handle() -> Self;
    pub fn native_registration_failed(detail: Option<String>) -> Self;
    pub fn duplicate_registration() -> Self;
    pub fn registration_failure_mode(&self) -> Option<RegistrationFailureMode>;
}
```

### Integration Contract

```rust
// In code-base/engine-texture-registry:
match Texture::new_with_provider(engine_handle, provider) {
    Err(e) => match e.registration_failure_mode() {
        Some(RegistrationFailureMode::InvalidThread) => {
            // Log: must be called on Platform Thread
        }
        Some(RegistrationFailureMode::InvalidHandle) => {
            // Engine was destroyed; clean up registration state
        }
        Some(RegistrationFailureMode::NativeRegistrationFailed) => {
            // Platform-specific failure; report with detail
        }
        _ => {}
    }
    Ok(texture) => { /* ... */ }
}
```

---

## F-02: Android Payload Path Contract

### Purpose

Expose when `get_payload()` is called relative to `mark_frame_available()` to allow higher-level code to reason about thread contracts.

### Changes

**File**: `texture/src/lib.rs`

```rust
// NEW: PayloadTiming enum
pub enum PayloadTiming {
    PushDuringMarkAvailable,  // Android
    PullDuringRaster,         // Darwin
    NotApplicable,
}

// NEW: PayloadPathContract trait
pub trait PayloadPathContract {
    fn payload_timing() -> PayloadTiming;
}
```

**File**: `texture/src/platform/android/mod.rs`

```rust
impl PayloadPathContract for BoxedPixelData {
    fn payload_timing() -> PayloadTiming {
        PayloadTiming::PushDuringMarkAvailable
    }
}
```

**File**: `texture/src/platform/darwin/mod.rs`

```rust
impl PayloadPathContract for BoxedIOSurface {
    fn payload_timing() -> PayloadTiming {
        PayloadTiming::PullDuringRaster
    }
}
```

### Integration Contract

```rust
// In code-base/texture-adapter:
let timing = <BoxedPixelData as PayloadPathContract>::payload_timing();
match timing {
    PayloadTiming::PushDuringMarkAvailable => {
        // Android: get_payload() called on Platform Thread
        // Ensure SharedSource is ready for Platform Thread access
    }
    PayloadTiming::PullDuringRaster => {
        // Darwin: get_payload() called on Raster Thread
        // Ensure payload construction is lightweight
    }
    _ => {}
}
```

---

## F-03: iOS SurfaceCache Ownership

### Purpose

Allow explicit control over IOSurface caching behavior to preserve the `SharedSource` ownership model.

### Changes

**File**: `texture/src/lib.rs`

```rust
// NEW: SurfaceCachePolicy enum
pub enum SurfaceCachePolicy {
    CacheWithReuse,  // Default (backward compatible)
    NoCache,         // Fresh surface every time
    CacheNoClone,    // TODO: requires lifetime redesign
}
```

**File**: `texture/src/platform/darwin/mod.rs`

```rust
// MODIFIED: SurfaceCache with policy
pub struct SurfaceCache {
    policy: SurfaceCachePolicy,
    // ...
}

impl SurfaceCache {
    pub fn new(
        parent_provider: Arc<dyn PayloadProvider<BoxedIOSurface>>,
        update_requested: Arc<AtomicBool>,
        policy: SurfaceCachePolicy,  // NEW parameter
    ) -> Self;
}

// NEW: API with explicit policy
impl PlatformTexture<BoxedIOSurface> {
    pub fn new_with_policy(
        engine_handle: i64,
        provider: Arc<dyn PayloadProvider<BoxedIOSurface>>,
        policy: SurfaceCachePolicy,
    ) -> Result<Self>;

    // Backward compatible default
    pub fn new(...) -> Result<Self> {
        Self::new_with_policy(..., SurfaceCachePolicy::CacheWithReuse)
    }
}
```

### Integration Contract

```rust
// In code-base/platform-ios:
use irondash_texture::SurfaceCachePolicy;

pub fn create_texture(
    engine_handle: i64,
    provider: Arc<dyn PayloadProvider<BoxedIOSurface>>,
) -> Result<PlatformTexture<BoxedIOSurface>> {
    // Use NoCache to preserve SharedSource ownership model
    PlatformTexture::new_with_policy(
        engine_handle,
        provider,
        SurfaceCachePolicy::NoCache,
    )
}
```

---

## F-04: Android Zero-Copy Extension

### Purpose

Provide extension points for AHardwareBuffer/EGL-oriented integration without forcing the current CPU-copy path.

Follow-up design for the final no-copy delivery path: `F04_ANDROID_FINAL_ZERO_COPY_DELIVERY_DESIGN.md`.

### Changes

**File**: `texture/src/platform/android/mod.rs`

```rust
// NEW: AHardwareBufferHandle wrapper
pub struct AHardwareBufferHandle {
    buffer: *mut AHardwareBuffer,
}

impl AHardwareBufferHandle {
    pub unsafe fn from_raw(buffer: *mut AHardwareBuffer) -> Self;
    pub fn as_raw(&self) -> *mut AHardwareBuffer;
    pub unsafe fn acquire(&self);
    pub unsafe fn release(&self);
    pub unsafe fn describe(&self) -> AHardwareBuffer_Desc;
}

// NEW: AHardwareBufferProvider trait
pub trait AHardwareBufferProvider: Send + Sync {
    fn get_buffer(&self) -> AHardwareBufferHandle;
    fn get_width(&self) -> i32;
    fn get_height(&self) -> i32;
}

// NEW: final-path frame provider contract
pub enum AndroidHardwareBufferFormat {
    Rgba8888,
}

pub struct HardwareBufferFrame {
    pub buffer: AHardwareBufferHandle,
    pub width: i32,
    pub height: i32,
    pub format: AndroidHardwareBufferFormat,
    pub generation: u64,
    pub acquire_fence_fd: Option<OwnedFd>,
    pub release_token: u64,
}

pub struct HardwareBufferFrameRelease {
    pub release_token: u64,
    pub release_fence_fd: Option<OwnedFd>,
}

pub enum AcquireFrameOutcome {
    Acquired(HardwareBufferFrame),
    NoNewFrame { latest_generation: u64 },
}

pub trait AHardwareBufferFrameProvider: Send + Sync {
    fn acquire_latest_frame(&self, after_generation: u64) -> Result<AcquireFrameOutcome>;
    fn release_frame(&self, release: HardwareBufferFrameRelease) -> Result<()>;
}

// NEW: distinct final-path texture marker
pub struct ImportedHardwareBufferTexture;

// NEW: DeferredPayloadFlush trait
pub trait DeferredPayloadFlush {
    fn flush_payload(&self) -> Result<()>;
}
```

**File**: `texture/src/lib.rs`

```rust
// Re-export for Android builds
#[cfg(target_os = "android")]
pub use crate::platform::android::{
    AHardwareBufferFrameProvider,
    AHardwareBufferHandle,
    AHardwareBufferProvider,
    AcquireFrameOutcome,
    AndroidHardwareBufferFormat,
    DeferredPayloadFlush,
    HardwareBufferFrame,
    HardwareBufferFrameRelease,
    ImportedHardwareBufferTexture,
};
```

### Integration Contract

```rust
// In code-base/platform-android:
use irondash_texture::{AHardwareBufferProvider, AHardwareBufferHandle};

pub struct EglImageProvider {
    ahb: AHardwareBufferHandle,
    // ...
}

impl AHardwareBufferProvider for EglImageProvider {
    fn get_buffer(&self) -> AHardwareBufferHandle {
        self.ahb.clone()
    }
    fn get_width(&self) -> i32 { /* ... */ }
    fn get_height(&self) -> i32 { /* ... */ }
}

// Future: Zero-copy texture creation
let provider = Arc::new(EglImageProvider::new(ahb));
// TODO: Texture::new_with_ahardware_buffer(engine_handle, provider)
```

Current fork status after the 2026-04-23 follow-up is intentionally split in two layers:

- the public final-path contract now exists in code and is distinct from the seam path
- the actual consumer-import backend is still unavailable inside the fork, so `Texture::<ImportedHardwareBufferTexture>::new_with_hardware_buffer_frame_source(...)` currently fails with a precise backend-unavailable reason instead of silently reusing the seam path

---

## F-05: Engine Teardown Helper

### Purpose

Provide silent unregister path for engine teardown to avoid noisy failures.

### Changes

**File**: `texture/src/platform/darwin/mod.rs`

```rust
impl PlatformTexture<Type> {
    // MODIFIED: destroy() now silent on engine-destroyed
    fn destroy(&mut self) -> Result<()> {
        match Self::texture_registery(self.engine_handle) {
            Ok(mut registry) => {
                unsafe { registry.unregisterTexture(self.id) };
                Ok(())
            }
            Err(_) => {
                // Engine already destroyed; silent no-op
                Ok(())
            }
        }
    }

    // NEW: Explicit unregister for non-teardown scenarios
    pub fn unregister(&mut self) -> Result<()> {
        let mut registry = Self::texture_registery(self.engine_handle)?;
        unsafe { registry.unregisterTexture(self.id) };
        Ok(())
    }
}
```

**File**: `texture/src/platform/android/mod.rs`

```rust
impl PlatformTexture<Type> {
    // MODIFIED: destroy() now silent on thread attachment failure
    fn destroy(&mut self) -> Result<()> {
        match java_vm.attach_current_thread() {
            Ok(mut env) => {
                let _ = env.call_method(self.texture_entry.as_obj(), "release", "()V", &[]);
                unsafe { ANativeWindow_release(self.native_window); }
                Ok(())
            }
            Err(_) => {
                // Thread attachment failed; likely during teardown
                Ok(())
            }
        }
    }

    // NEW: Explicit unregister for non-teardown scenarios
    pub fn unregister(&mut self) -> Result<()> { /* ... */ }
}
```

### Integration Contract

```rust
// In code-base/ffi-api:
impl Drop for TextureRegistryEntry {
    fn drop(&mut self) {
        // Silent teardown; no noisy failures if engine is gone
        self.texture.destroy().ok_log();
    }
}

// For explicit unregister (e.g., source eviction):
texture.unregister()?;  // May fail; handle explicitly
```

---

## Acceptance Criteria Verification

### ✅ F-01 Acceptance

- [x] `RegistrationFailureMode` enum added
- [x] Error variants structured with mode + detail
- [x] Helper methods for common error creation
- [x] `From<EngineContextError>` maps to appropriate modes
- [x] Darwin and Android platforms use new error taxonomy

### ✅ F-02 Acceptance

- [x] `PayloadTiming` enum added
- [x] `PayloadPathContract` trait defined
- [x] Android `BoxedPixelData` → `PushDuringMarkAvailable`
- [x] Darwin `BoxedIOSurface` → `PullDuringRaster`
- [x] Re-exported in public API

### ✅ F-03 Acceptance

- [x] `SurfaceCachePolicy` enum added
- [x] `SurfaceCache` accepts policy parameter
- [x] `PlatformTexture::new_with_policy()` API added
- [x] Default `new()` preserves backward compatibility
- [x] All three policies documented

### ✅ F-04 Acceptance

- [x] `AHardwareBufferHandle` wrapper added
- [x] `AHardwareBufferProvider` trait defined
- [x] `AHardwareBufferFrameProvider` final-path trait defined
- [x] `ImportedHardwareBufferTexture` distinct final-path marker added
- [x] `DeferredPayloadFlush` trait defined
- [x] Re-exported for Android builds
- [x] Import path now fails at a precise backend boundary instead of generic unsupported

### ✅ F-05 Acceptance

- [x] `destroy()` silent on engine-destroyed
- [x] `unregister()` explicit API added
- [x] Darwin and Android platforms consistent
- [x] Teardown path avoids noisy failures

---

## Downstream Integration Checklist

### For `engine-texture-registry`

- [ ] Use `RegistrationFailureMode` for error handling
- [ ] Handle `InvalidHandle` as engine-destroyed signal
- [ ] Log `InvalidThread` as contract violation

### For `texture-adapter`

- [ ] Query `PayloadPathContract::payload_timing()` during bind
- [ ] Prepare for platform-specific payload fetch timing
- [ ] Document thread assumptions per platform

### For `platform-ios`

- [ ] Use `SurfaceCachePolicy::NoCache` explicitly
- [ ] Document IOSurface ownership flow
- [ ] Verify retain count behavior in tests

### For `platform-android`

- [ ] Implement `AHardwareBufferProvider` for EGL path
- [ ] Implement the actual consumer-import substrate behind `ImportedHardwareBufferTexture`
- [ ] Use `DeferredPayloadFlush` if normalizing to pull model
- [ ] Preserve CPU-copy path for backward compatibility

### For `ffi-api`

- [ ] Rely on silent `destroy()` during teardown
- [ ] Use explicit `unregister()` for controlled eviction
- [ ] Handle `RegistrationFailureMode` in FFI error mapping

---

## Future Work (Wave 2)

### F-04 Follow-up: Full Zero-Copy Implementation

Reference design: `F04_ANDROID_FINAL_ZERO_COPY_DELIVERY_DESIGN.md`

When `platform-android` crate is ready:

1. Implement `Texture::new_with_ahardware_buffer()` in fork
2. Add EGL Image creation hook
3. Integrate with `platform-android` crate

### F-03 Follow-up: CacheNoClone Implementation

If lifetime redesign is warranted:

1. Add lifetime parameter to `PayloadProvider`
2. Implement borrowed reference return
3. Update `SurfaceCache` to avoid clone

### F-01 Follow-up: Duplicate Registration Detection

If needed by `engine-texture-registry`:

1. Add per-engine registration tracking
2. Return `DuplicateRegistration` error proactively
3. Document idempotency rules

---

## Maintenance Notes

### Fork Sync Strategy

When syncing with upstream `irondash`:

1. Create feature branch from upstream
2. Apply these modifications as patches
3. Resolve conflicts in:
   - `texture/src/error.rs`
   - `texture/src/lib.rs`
   - `texture/src/platform/darwin/mod.rs`
   - `texture/src/platform/android/mod.rs`
4. Run integration tests with `code-base`

### Patch Documentation

Each modification is marked with comments:

```rust
// ============================================================================
// F-XX: [Feature Name]
// ============================================================================
// Description and reference to this document
```

---

## Sign-Off

| Role | Name | Date |
| :--- | :--- | :--- |
| Tech Lead | | 2026-03-29 |
| engine-integration-specialist | | 2026-03-29 |
| platform-android-specialist | | 2026-03-29 |
| platform-ios-specialist | | 2026-03-29 |
| ffi-specialist | | 2026-03-29 |

---

**Wave 1 Complete**. Ready for `code-base` integration.
