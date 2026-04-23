#![allow(clippy::new_without_default)]
#![allow(clippy::type_complexity)]

use std::sync::{Arc, Mutex};

use irondash_run_loop::{util::Capsule, RunLoop, RunLoopSender};
use platform::PlatformTexture;

mod error;
mod log;
mod platform;

pub use error::*;

pub type Result<T> = std::result::Result<T, Error>;

/// Native texture.
///
/// `Type` parameters specifies the payload type of the texture.
/// It can be [`BoxedPixelData`], which is supported on all platforms, or
/// one of the platform specific types such as `BoxedIOSurface`,
/// `BoxedGLTexture` or `BoxedTextureDescriptor`.
pub struct Texture<Type> {
    platform_texture: PlatformTexture<Type>,
}

impl<Type> Texture<Type> {
    /// Returns identifier of the texture. This needs to be passed to
    /// Dart and used to create a Flutter Texture widget.
    pub fn id(&self) -> i64 {
        self.platform_texture.id()
    }

    /// Informs Flutter that new texture frame is available.
    /// This will make Flutter request new texture payload from provider
    /// during next frame rasterization.
    pub fn mark_frame_available(&self) -> Result<()> {
        self.platform_texture.mark_frame_available()
    }

    /// Converts Texture to a SendableTexture. SendableTexture can be
    /// sent between threads and update the content on any thread.
    pub fn into_sendable_texture(self) -> Arc<SendableTexture<Type>> {
        Arc::new(SendableTexture {
            sender: RunLoop::current().new_sender(),
            texture: Mutex::new(Capsule::new(self)),
        })
    }
}

///
/// Trait that implemented by objects that provide texture contents.
pub trait PayloadProvider<Type>: Send + Sync {
    /// Called by the engine to get the latest texture payload. This will
    /// most likely be called on raster thread. Hence PayloadProvider must
    /// be thread safe.
    ///
    /// Boxed payload is used to allow custom payload objects, which might
    /// be useful in situation where the provider needs to know when Flutter
    /// is done with the payload (i.e. by implementing Drop trait on the payload
    /// object).
    fn get_payload(&self) -> Type;
}

impl<Type: PlatformTextureWithProvider> Texture<Type> {
    /// Creates new texture for given engine with specified payload provider.
    ///
    /// Creating PixelData backed texture is supported on all platforms:
    ///
    /// ```ignore
    /// // Assume MyPixelDataProvider implements PayloadProvider<BoxedPixelData>
    /// let provider = Arc::new(MyPixelDataProvider::new());
    ///
    /// let texture = Texture::new_with_provider(engine_handle, provider)?;
    ///
    /// // This will cause flutter to request a PixelData during next
    /// // frame rasterization.
    /// texture.mark_frame_available()?;
    /// ```
    pub fn new_with_provider(
        engine_handle: i64,
        payload_provider: Arc<dyn PayloadProvider<Type>>,
    ) -> Result<Self> {
        Ok(Self {
            platform_texture: Type::create_texture(engine_handle, payload_provider)?,
        })
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl Texture<BoxedIOSurface> {
    /// Creates a new Darwin texture with an explicit IOSurface cache policy.
    ///
    /// This keeps `new_with_provider` backward compatible while allowing
    /// SharedSource-backed paths to opt out of irondash surface caching.
    pub fn new_with_provider_and_policy(
        engine_handle: i64,
        payload_provider: Arc<dyn PayloadProvider<BoxedIOSurface>>,
        policy: SurfaceCachePolicy,
    ) -> Result<Self> {
        Ok(Self {
            platform_texture: PlatformTexture::<BoxedIOSurface>::new_with_policy(
                engine_handle,
                payload_provider,
                policy,
            )?,
        })
    }
}

impl<Type: PlatformTextureWithoutProvider> Texture<Type> {
    /// Creates new texture for given engine without payload. This is used on
    /// Android where instead of providing payload to the texture,
    /// you work directly with underlying surface or native window.
    ///
    /// ```ignore
    /// let texture = Texture::<NativeWindow>::new(engine_handle)?;
    /// let native_window = texture.get();
    /// ```
    pub fn new(engine_handle: i64) -> Result<Self> {
        Ok(Self {
            platform_texture: Type::create_texture(engine_handle)?,
        })
    }

    pub fn get(&self) -> Type {
        Type::get(&self.platform_texture)
    }
}

#[cfg(target_os = "android")]
impl Texture<NativeWindow> {
    /// Creates a new Android texture and retains a hardware-buffer-backed
    /// source for hardware-buffer-backed frame delivery.
    ///
    /// `mark_frame_available()` will flush the retained hardware-buffer source
    /// into the engine-local texture on the Platform Thread. Callers that want
    /// explicit control can also use `DeferredPayloadFlush::flush_payload()`.
    pub fn new_with_hardware_buffer_source(
        engine_handle: i64,
        source: AndroidHardwareBufferTextureSource,
    ) -> Result<Self> {
        Ok(Self {
            platform_texture: PlatformTexture::<NativeWindow>::new_with_hardware_buffer_source(
                engine_handle,
                source,
            )?,
        })
    }
}

#[cfg(target_os = "android")]
impl Texture<ImportedHardwareBufferTexture> {
    /// Creates a distinct Android final-path texture registration backed by an
    /// explicit frame-provider contract.
    pub fn new_with_hardware_buffer_frame_source(
        engine_handle: i64,
        provider: Arc<dyn AHardwareBufferFrameProvider>,
    ) -> Result<Self> {
        Ok(Self {
            platform_texture:
                PlatformTexture::<ImportedHardwareBufferTexture>::new_with_hardware_buffer_frame_source(
                    engine_handle,
                    provider,
                )?,
        })
    }
}

#[cfg(target_os = "android")]
impl DeferredPayloadFlush for Texture<NativeWindow> {
    fn flush_payload(&self) -> Result<()> {
        self.platform_texture.flush_payload()
    }
}

pub enum PixelFormat {
    BGRA,
    RGBA,
}

/// Pixel data is supported payload type on every platform, but the expected
/// PixelFormat may differ. You can [`PixelData::FORMAT`] to query expected
/// pixel format.
pub struct PixelData<'a> {
    pub width: i32,
    pub height: i32,
    pub data: &'a [u8],
}

impl PixelData<'_> {
    pub const FORMAT: PixelFormat = platform::PIXEL_DATA_FORMAT;
}

pub trait PixelDataProvider {
    fn get(&self) -> PixelData<'_>;
}

/// Actual type for pixel buffer payload.
pub type BoxedPixelData = Box<dyn PixelDataProvider>;

/// Convenience implementation for pixel data texture.
pub struct SimplePixelData {
    width: i32,
    height: i32,
    data: Vec<u8>,
}

impl SimplePixelData {
    pub fn new_boxed(width: i32, height: i32, data: Vec<u8>) -> Box<Self> {
        Box::new(Self {
            width,
            height,
            data,
        })
    }
}

impl PixelDataProvider for SimplePixelData {
    fn get(&self) -> PixelData<'_> {
        PixelData {
            width: self.width,
            height: self.height,
            data: &self.data,
        }
    }
}

//
// Platform specific payloads.
//

#[cfg(target_os = "android")]
mod android {
    // These can be obtained from texture using Texture::get(&self).
    pub type NativeWindow = super::platform::NativeWindow;
    pub type Surface = super::platform::Surface;
}
#[cfg(target_os = "android")]
pub use android::*;

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod darwin {
    pub mod io_surface {
        pub use crate::platform::io_surface::*;
    }

    pub trait IOSurfaceProvider {
        fn get(&self) -> &io_surface::IOSurface;
    }

    /// Payload type for IOSurface backed texture.
    pub type BoxedIOSurface = Box<dyn IOSurfaceProvider>;
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use darwin::*;

#[cfg(target_os = "linux")]
mod linux {
    pub struct GLTexture<'a> {
        pub target: u32,   // texture target (i.e. GL_TEXTURE_2D or GL_TEXTURE_RECTANGLE)
        pub name: &'a u32, // OpenGL texture name
        pub width: i32,
        pub height: i32,
    }

    pub trait GLTextureProvider {
        fn get(&self) -> GLTexture;
    }

    /// Payload type for IOSurface backed texture.
    pub type BoxedGLTexture = Box<dyn GLTextureProvider>;
}

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "windows")]
mod windows {
    use std::ffi::c_void;

    /// Texture descriptor for native texture.
    pub struct TextureDescriptor<'a, HandleType> {
        pub handle: &'a HandleType,
        pub width: i32,
        pub height: i32,
        pub visible_width: i32,
        pub visible_height: i32,
        pub pixel_format: super::PixelFormat,
    }

    pub trait TextureDescriptorProvider<HandleType> {
        fn get(&self) -> TextureDescriptor<HandleType>;
    }

    pub type BoxedTextureDescriptor<HandleType> = Box<dyn TextureDescriptorProvider<HandleType>>;

    /// Wrapper around `ID3D11Texture2D`, can be used as `TextureHandle` in
    /// `TextureDescriptor`.
    pub struct ID3D11Texture2D(pub *mut c_void);

    /// Wrapper around DXGI shared handle (*mut HANDLE), can be used as
    // `TextureHandle` in `TextureDescriptor`.
    pub struct DxgiSharedHandle(pub *mut c_void);
}
#[cfg(target_os = "windows")]
pub use windows::*;

// ============================================================================
// F-02: Android Payload Path Contract
// ============================================================================
// This trait allows higher-level code to reason about when get_payload()
// will be called relative to mark_frame_available().
//
// See: code-base/IRONDASH_FORK_WORKBOOK.md §7 (F-02)

/// Describes the payload fetch timing contract for a platform.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PayloadTiming {
    /// Payload fetched during mark_frame_available() (Android current behavior)
    PushDuringMarkAvailable,
    /// Payload fetched later via callback (Darwin current behavior)
    PullDuringRaster,
    /// This payload type does not use get_payload() (e.g., NativeWindow direct access)
    NotApplicable,
}

/// Trait for querying payload timing behavior.
///
/// # Example
/// ```ignore
/// use irondash_texture::{PayloadPathContract, PayloadTiming, BoxedPixelData};
///
/// let timing = <BoxedPixelData as PayloadPathContract>::payload_timing();
/// match timing {
///     PayloadTiming::PushDuringMarkAvailable => {
///         // Android: prepare for Platform Thread payload fetch
///     }
///     PayloadTiming::PullDuringRaster => {
///         // Darwin: prepare for Raster Thread payload fetch
///     }
///     _ => {}
/// }
/// ```
pub trait PayloadPathContract {
    /// Returns when get_payload() is called relative to mark_frame_available().
    fn payload_timing() -> PayloadTiming;
}

// ============================================================================
// F-03: iOS SurfaceCache Ownership Policy
// ============================================================================
// This enum allows higher-level code to control SurfaceCache behavior on Darwin.
//
// See: code-base/IRONDASH_FORK_WORKBOOK.md §7 (F-03)

/// Controls how SurfaceCache manages IOSurface caching and ownership on Darwin.
///
/// # Policies
/// - `CacheWithReuse`: Cache surfaces for reuse (default, backward compatible)
/// - `NoCache`: Fresh surface every time (clear ownership, recommended for SharedSource)
/// - `CacheNoClone`: Cache but return borrowed reference (requires lifetime redesign)
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum SurfaceCachePolicy {
    /// Cache surfaces for reuse. May increment retain count on clone.
    /// Use when performance is critical and retain count is managed externally.
    #[default]
    CacheWithReuse,

    /// No caching. Fresh surface from provider every time.
    /// Use when ownership clarity is critical (e.g., SharedSource model).
    NoCache,

    /// Cache but never clone. Return borrowed reference.
    /// Use when caller manages lifetime explicitly.
    /// Note: This variant requires lifetime redesign and is not yet implemented.
    CacheNoClone,
}

// ============================================================================
// F-04: Android Zero-Copy Extension Seam
// ============================================================================
// Re-export Android-specific zero-copy types when building for Android.
//
// See: code-base/IRONDASH_FORK_WORKBOOK.md §7 (F-04)

#[cfg(target_os = "android")]
mod android_zero_copy {
    pub use crate::platform::{
        AHardwareBufferFrameProvider, AHardwareBufferHandle, AHardwareBufferProvider,
        AcquireFrameOutcome, AndroidHardwareBufferFormat, AndroidHardwareBufferSourceKind,
        AndroidHardwareBufferTextureSource, DeferredPayloadFlush, HardwareBufferFrame,
        HardwareBufferFrameRelease, ImportedHardwareBufferTexture,
    };
}

#[cfg(target_os = "android")]
pub use android_zero_copy::*;

// ============================================================================

use crate::log::OkLog;

/// SendableTexture is Send and Sync so it can be sent between threads, but it
/// can only update the texture, it can not retrieve payload (such as Surface
/// or NativeWindow on Android).
pub struct SendableTexture<T: 'static> {
    sender: RunLoopSender,
    texture: Mutex<Capsule<Texture<T>>>,
}

impl<T> SendableTexture<T> {
    pub fn mark_frame_available(self: &Arc<Self>) {
        if self.sender.is_same_thread() {
            let texture = self.texture.lock().unwrap();
            let texture = texture.get_ref().unwrap();
            texture.mark_frame_available().ok_log();
        } else {
            let texture_clone = self.clone();
            self.sender.send(move || {
                let texture = texture_clone.texture.lock().unwrap();
                let texture = texture.get_ref().unwrap();
                texture.mark_frame_available().ok_log();
            });
        }
    }
}

// Helper traits

pub trait PlatformTextureWithProvider: Sized {
    fn create_texture(
        engine_handle: i64,
        payload_provider: Arc<dyn PayloadProvider<Self>>,
    ) -> Result<PlatformTexture<Self>>;
}

pub trait PlatformTextureWithoutProvider: Sized {
    fn create_texture(engine_handle: i64) -> Result<PlatformTexture<Self>>;

    fn get(texture: &PlatformTexture<Self>) -> Self;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_cache_policy_default_remains_cache_with_reuse() {
        assert_eq!(
            SurfaceCachePolicy::default(),
            SurfaceCachePolicy::CacheWithReuse
        );
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    #[test]
    fn darwin_policy_constructor_is_exposed() {
        let _constructor: fn(
            i64,
            std::sync::Arc<dyn PayloadProvider<BoxedIOSurface>>,
            SurfaceCachePolicy,
        ) -> Result<Texture<BoxedIOSurface>> = Texture::<BoxedIOSurface>::new_with_provider_and_policy;
    }
}
