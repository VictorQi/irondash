# F-03: iOS SurfaceCache Ownership Clarification

> **Status**: Design Complete  
> **Priority**: P0 (Blocks Gate 2 `platform-ios` and Gate 3 `texture-adapter`)  
> **Owner**: platform-ios-specialist + engine-integration-specialist  
> **Target**: `irondash/texture/src/platform/darwin/mod.rs`

---

## 1. Problem Statement

### 1.1 Current Behavior (Pre-F-03)

The Darwin `SurfaceCache` implementation in `irondash/texture` exhibits the following behavior:

```rust
// In SurfaceCache::get_payload():
pub fn get_payload(&self) -> BoxedIOSurface {
    let mut surface = self.surface.lock().unwrap();
    if self.update_requested.load(Ordering::Acquire) {
        surface.take();  // ← Clear cached surface
        self.update_requested.store(false, Ordering::Release);
    }
    // Clone the IOSurface (may increment retain count)
    let surface = surface.get_or_insert_with(|| {
        self.parent_provider.get_payload().get().clone()
    });
    Box::new(IOSurfaceHolder {
        surface: surface.clone(),  // ← Another clone
    })
}
```

### 1.2 Ownership Ambiguity

The design document `platform-ios/DESIGN.md §3` specifies:

> **C1 (Single Retain)**: The primary retain of `IOSurfaceRef` must happen exactly once
> in Swift/ObjC side during `CVPixelBufferGetIOSurface()`. Rust side must not add
> additional retains beyond the single `CFRetain` during creation.

The current `SurfaceCache` behavior creates ambiguity:

1. `surface.clone()` is called **twice** per `get_payload()`:
   - Once to cache in `SurfaceCache::surface`
   - Once to return in `IOSurfaceHolder`
2. Each `IOSurface::clone()` may increment the retain count (depends on `TCFType` implementation)
3. `SurfaceAdapter::surface_for_pixel_buffer()` also caches and clones

### 1.3 N-02 Issue Connection

The N-02 issue in `INDEX.md` states:

> **N-02 (P2)**: `irondash` SurfaceCache and SharedSource DeferredDrop lifecycle tension.
> If `SurfaceCache` becomes the effective owner via repeated retains, the `SharedSource`
> ownership model breaks.

---

## 2. Root Cause Analysis

### 2.1 SurfaceCache Original Intent

From the code comment:

```rust
/// SurfaceCache has two purposes:
/// 1. It makes sure we keep onto the payload while surface is in use.
/// 2. On iOS, which has a bug that requests the texture during every frame
///    regardless of mark_frame_available this reuses existing surface until next
///    call to mark_frame_available.
```

**Purpose 1** is valid: keeping payload alive during use.

**Purpose 2** works around an iOS Flutter bug but conflicts with the ownership model.

### 2.2 Retain Count Implications

```
Call chain:
1. parent_provider.get_payload() → returns BoxedIOSurface (1 retain from CVPixelBufferCreateWithIOSurface)
2. surface.get_or_insert_with(...clone()) → SurfaceCache holds clone (retain +1?)
3. surface.clone() → returned to Flutter (retain +1?)

Total retains: 1 (creation) + ? (cache) + ? (return)
```

If each `clone()` increments retain, the single-retain contract is violated.

### 2.3 SurfaceAdapter Additional Caching

```rust
// In SurfaceAdapter::surface_for_pixel_buffer():
fn surface_for_pixel_buffer(&self, width: i32, height: i32) -> IOSurface {
    let mut cached_surface = self.cached_surface.lock().unwrap();
    if let Some(cached_surface) = cached_surface.as_ref() {
        // Reuse if dimensions match (another clone)
        return cached_surface.clone();
    }
    // ... create new surface ...
}
```

This adds a **third** caching layer with additional clones.

---

## 3. Design Decision

### 3.1 Option Analysis

| Option | Description | Pros | Cons |
| :----- | :---------- | :--- | :--- |
| A | Remove all caching; fresh surface every time | Clear ownership; no retain ambiguity | Performance regression; iOS bug workaround lost |
| B | Single cache with explicit ownership semantics | Preserves performance; clarifies ownership | Moderate fork changes |
| C | Add ownership policy enum for cache behavior | Flexible; allows project-specific tuning | Adds complexity; may confuse users |

### 3.2 Selected Approach: **Option B with Policy Hook**

**Rationale**:
- Preserves iOS bug workaround (Purpose 2)
- Clarifies ownership: `SurfaceCache` is a **borrower**, not owner
- Allows `code-base` to opt into stricter ownership if needed

---

## 4. Substrate Modification Specification

### 4.1 New Trait: `SurfaceCachePolicy`

Add to `irondash/texture/src/platform/darwin/mod.rs`:

```rust
/// Controls how SurfaceCache manages IOSurface caching and ownership.
/// 
/// This policy allows higher-level code to choose between:
/// - Performance (caching with potential retain ambiguity)
/// - Ownership clarity (no caching, fresh surfaces)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SurfaceCachePolicy {
    /// Cache surfaces for reuse. May increment retain count on clone.
    /// Use when performance is critical and retain count is managed externally.
    CacheWithReuse,
    
    /// No caching. Fresh surface from provider every time.
    /// Use when ownership clarity is critical (e.g., SharedSource model).
    NoCache,
    
    /// Cache but never clone. Return borrowed reference.
    /// Use when caller manages lifetime explicitly.
    CacheNoClone,
}
```

### 4.2 SurfaceCache with Policy

Modify `SurfaceCache` to accept policy:

```rust
pub struct SurfaceCache {
    surface: Mutex<Option<IOSurface>>,
    parent_provider: Arc<dyn PayloadProvider<BoxedIOSurface>>,
    update_requested: Arc<AtomicBool>,
    policy: SurfaceCachePolicy,  // ← New field
}

impl SurfaceCache {
    pub fn new(
        parent_provider: Arc<dyn PayloadProvider<BoxedIOSurface>>,
        update_requested: Arc<AtomicBool>,
        policy: SurfaceCachePolicy,  // ← New parameter
    ) -> Self {
        Self {
            surface: Mutex::new(None),
            parent_provider,
            update_requested,
            policy,
        }
    }
}

impl PayloadProvider<BoxedIOSurface> for SurfaceCache {
    fn get_payload(&self) -> BoxedIOSurface {
        match self.policy {
            SurfaceCachePolicy::CacheWithReuse => {
                // Current behavior: cache and clone
                let mut surface = self.surface.lock().unwrap();
                if self.update_requested.load(Ordering::Acquire) {
                    surface.take();
                    self.update_requested.store(false, Ordering::Release);
                }
                let surface = surface.get_or_insert_with(|| {
                    self.parent_provider.get_payload().get().clone()
                });
                Box::new(IOSurfaceHolder {
                    surface: surface.clone(),
                })
            }
            SurfaceCachePolicy::NoCache => {
                // Fresh surface every time: no caching, single clone
                Box::new(IOSurfaceHolder {
                    surface: self.parent_provider.get_payload().get().clone(),
                })
            }
            SurfaceCachePolicy::CacheNoClone => {
                // Cache but return borrowed reference (requires lifetime gymnastics)
                // This may need API redesign; placeholder for now
                todo!("CacheNoClone requires lifetime redesign")
            }
        }
    }
}
```

### 4.3 SurfaceAdapter with Policy

Similarly update `SurfaceAdapter`:

```rust
pub struct SurfaceAdapter {
    pixel_provider: Arc<dyn PayloadProvider<BoxedPixelData>>,
    cached_surface: Mutex<Option<IOSurface>>,
    policy: SurfaceCachePolicy,  // ← New field
}

impl SurfaceAdapter {
    pub fn new(
        pixel_provider: Arc<dyn PayloadProvider<BoxedPixelData>>,
        policy: SurfaceCachePolicy,  // ← New parameter
    ) -> Self {
        Self {
            pixel_provider,
            cached_surface: Mutex::new(None),
            policy,
        }
    }
    
    fn surface_for_pixel_buffer(&self, width: i32, height: i32) -> IOSurface {
        match self.policy {
            SurfaceCachePolicy::CacheWithReuse => {
                // Current caching behavior
                let mut cached_surface = self.cached_surface.lock().unwrap();
                if let Some(cached_surface) = cached_surface.as_ref() {
                    unsafe {
                        let surface = cached_surface.as_concrete_TypeRef();
                        if IOSurfaceGetWidth(surface) == width as usize
                            && IOSurfaceGetHeight(surface) == height as usize
                        {
                            return cached_surface.clone();
                        }
                    }
                }
                let surface = init_surface(width, height);
                cached_surface.replace(surface.clone());
                surface
            }
            SurfaceCachePolicy::NoCache => {
                // Fresh surface every time
                init_surface(width, height)
            }
            SurfaceCachePolicy::CacheNoClone => {
                todo!("CacheNoClone requires lifetime redesign")
            }
        }
    }
}
```

### 4.4 Texture Creation API Update

Update `PlatformTexture::new` to accept policy:

```rust
impl<Type> PlatformTexture<Type> {
    pub fn new_with_policy(
        engine_handle: i64,
        provider: Arc<dyn PayloadProvider<BoxedIOSurface>>,
        policy: SurfaceCachePolicy,  // ← New parameter
    ) -> Result<Self> {
        let update_requested = Arc::new(AtomicBool::new(false));
        let provider = Arc::new(SurfaceCache::new(provider, update_requested.clone(), policy));
        let texture_objc = IrondashTexture::new_with_provider(provider);
        let mut texture_registry = Self::texture_registery(engine_handle)?;
        let id: i64 = unsafe { texture_registry.registerTexture(&texture_objc) };
        Ok(Self {
            id,
            engine_handle,
            _texture_objc: texture_objc,
            _phantom: PhantomData,
            update_requested,
        })
    }
    
    // Keep existing new() as default (CacheWithReuse for backward compatibility)
    pub fn new(
        engine_handle: i64,
        provider: Arc<dyn PayloadProvider<BoxedIOSurface>>,
    ) -> Result<Self> {
        Self::new_with_policy(engine_handle, provider, SurfaceCachePolicy::CacheWithReuse)
    }
}
```

---

## 5. Integration Contract for `code-base`

### 5.1 `platform-ios` Usage

```rust
// In platform-ios/src/ios_platform_cleaner.rs:
use irondash_texture::{SurfaceCachePolicy, BoxedIOSurface, PayloadProvider};

pub struct IosPlatformCleaner {
    // ...
}

impl IosPlatformCleaner {
    /// Create IOSurface-backed texture with explicit ownership policy.
    /// 
    /// We use NoCache to preserve the SharedSource ownership model:
    /// - SharedSource is the single owner of the IOSurfaceRef
    /// - SurfaceCache is a borrower, not an owner
    /// - CFRelease happens exactly once in DeferredDrop
    pub fn create_texture(
        engine_handle: i64,
        provider: Arc<dyn PayloadProvider<BoxedIOSurface>>,
    ) -> Result<PlatformTexture<BoxedIOSurface>> {
        PlatformTexture::new_with_policy(
            engine_handle,
            provider,
            SurfaceCachePolicy::NoCache,  // ← Explicit policy choice
        )
    }
}
```

### 5.2 `shared-source` Ownership Contract

Document the ownership flow:

```rust
// In shared-source/src/lib.rs:
/// ## IOSurface Ownership Flow
/// 
/// ```text
/// 1. Swift/ObjC: CVPixelBufferCreateWithIOSurface() → 1 retain (owner: SharedSource)
/// 2. Rust: Arc<SharedSource> holds the IOSurfaceRef
/// 3. Darwin: SurfaceCache with NoCache policy → borrows, doesn't retain
/// 4. Flutter: copyPixelBuffer receives borrowed reference
/// 5. Drop: DeferredDrop → CFRelease on GCD main queue (single release)
/// ```
/// 
/// **Invariant**: Exactly one CFRetain (creation) and one CFRelease (DeferredDrop).
```

---

## 6. Acceptance Criteria

### 6.1 Fork Changes

- [ ] `SurfaceCachePolicy` enum added to `irondash/texture/src/platform/darwin/mod.rs`
- [ ] `SurfaceCache::new()` accepts policy parameter
- [ ] `SurfaceAdapter::new()` accepts policy parameter
- [ ] `PlatformTexture::new_with_policy()` API added
- [ ] `PlatformTexture::new()` defaults to `CacheWithReuse` (backward compatible)
- [ ] All three policies (`CacheWithReuse`, `NoCache`, `CacheNoClone`) documented

### 6.2 `code-base` Integration

- [ ] `platform-ios` uses `SurfaceCachePolicy::NoCache` explicitly
- [ ] `shared-source` documents IOSurface ownership flow
- [ ] Tests verify retain count behavior per policy
- [ ] Integration test: two-engine shared texture path works with `NoCache`

---

## 7. Testing Strategy

### 7.1 Retain Count Verification

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn no_cache_policy_does_not_increment_retain_beyond_creation() {
        // Create mock IOSurface with known retain count (1 after creation)
        let mock_surface = MockIOSurface::new();
        let initial_retain = mock_surface.retain_count();
        
        // Create provider that returns mock surface
        let provider = Arc::new(MockProvider::new(mock_surface));
        
        // Create SurfaceCache with NoCache policy
        let cache = SurfaceCache::new(
            provider,
            Arc::new(AtomicBool::new(false)),
            SurfaceCachePolicy::NoCache,
        );
        
        // Call get_payload multiple times
        for _ in 0..5 {
            let _payload = cache.get_payload();
        }
        
        // Retain count should still be 1 (no additional retains from caching)
        assert_eq!(mock_surface.retain_count(), initial_retain);
    }
    
    #[test]
    fn cache_with_reuse_policy_may_increment_retain() {
        // Similar test but expect retain count to increase
        // (exact behavior depends on IOSurface::clone implementation)
    }
}
```

### 7.2 Two-Engine Shared Texture Test

```rust
#[test]
fn two_engines_share_same_source_with_no_cache_policy() {
    // Create SharedSource with single IOSurfaceRef
    let shared_source = Arc::new(SharedSource::new(/* ... */));
    
    // Bind to Engine A with NoCache policy
    let adapter_a = SharedTextureAdapter::bind(shared_source.clone(), engine_a);
    
    // Bind to Engine B with NoCache policy
    let adapter_b = SharedTextureAdapter::bind(shared_source.clone(), engine_b);
    
    // Both adapters should reference the same underlying IOSurface
    assert_eq!(adapter_a.surface_id(), adapter_b.surface_id());
    
    // Drop adapter A
    drop(adapter_a);
    
    // Adapter B should still work (SharedSource not released)
    let payload_b = adapter_b.get_payload();
    assert!(payload_b.is_valid());
    
    // Drop adapter B
    drop(adapter_b);
    
    // Now SharedSource should be released (DeferredDrop scheduled)
}
```

---

## 8. Risk Assessment

| Risk | Probability | Impact | Mitigation |
| :--- | :--------- | :----- | :--------- |
| Breaking existing iOS behavior | Low | High | Default policy is `CacheWithReuse` (backward compatible) |
| Performance regression with NoCache | Medium | Medium | Document trade-off; allow per-use-case policy selection |
| `CacheNoClone` requires lifetime redesign | High | Low | Mark as `todo!`; not needed for Gate 2/3 |
| Confusion from policy enum | Medium | Low | Clear documentation; examples in `platform-ios` |

---

## 9. Implementation Plan

### Phase 1: Policy Addition (Immediate)
1. Add `SurfaceCachePolicy` enum to `irondash/texture`
2. Update `SurfaceCache` and `SurfaceAdapter` to accept policy
3. Add `PlatformTexture::new_with_policy()` API
4. Keep `PlatformTexture::new()` as default (backward compatible)

### Phase 2: `code-base` Integration (Gate 2)
1. `platform-ios` explicitly uses `SurfaceCachePolicy::NoCache`
2. Document ownership flow in `shared-source`
3. Add retain count tests

### Phase 3: Optional Enhancement (Post-Gate 3)
1. Implement `CacheNoClone` if lifetime redesign is warranted
2. Add performance benchmarks comparing policies
3. Consider making `NoCache` the default for new projects

---

## 10. Connection to N-02 Issue

From `INDEX.md`:

> **N-02 (P2)**: irondash SurfaceCache and SharedSource DeferredDrop lifecycle tension.

**Resolution via F-03**:

With `SurfaceCachePolicy::NoCache`:
- `SurfaceCache` is explicitly a **borrower**, not owner
- `SharedSource` remains the single owner of `IOSurfaceRef`
- `DeferredDrop` is the single release path
- No tension: clear ownership boundary

---

**F-03 Design Complete**. Ready for implementation.
