use std::{cell::RefCell, marker::PhantomData, slice, sync::Arc};

use irondash_engine_context::EngineContext;
use jni::objects::{GlobalRef, JObject};
use ndk_sys::{
    AHardwareBuffer, AHardwareBuffer_Format, AHardwareBuffer_Desc,
    ANativeWindow, ANativeWindow_Buffer, ANativeWindow_acquire,
    ANativeWindow_fromSurface, ANativeWindow_lock, ANativeWindow_release,
    ANativeWindow_setBuffersGeometry, ANativeWindow_unlockAndPost,
};

// ============================================================================
// F-04: Android Zero-Copy Extension Seam
// ============================================================================
// This module provides extension points for AHardwareBuffer/EGL-oriented
// integration. The current CPU-copy pixel path remains for backward
// compatibility, but higher-level crates can opt into zero-copy via
// these extension traits.
//
// See: code-base/IRONDASH_FORK_WORKBOOK.md §7 (F-04)

/// Represents an AHardwareBuffer handle for zero-copy texture integration.
///
/// This is a transparent wrapper around `*mut AHardwareBuffer` that provides
/// safe acquisition and release semantics.
///
/// # Safety
/// The underlying AHardwareBuffer must be valid and have at least one
/// reference when this type is created.
pub struct AHardwareBufferHandle {
    buffer: *mut AHardwareBuffer,
}

unsafe impl Send for AHardwareBufferHandle {}
unsafe impl Sync for AHardwareBufferHandle {}

impl AHardwareBufferHandle {
    /// Creates a new handle from a raw pointer. Takes ownership of the reference.
    ///
    /// # Safety
    /// The pointer must be a valid `*mut AHardwareBuffer` with at least one reference.
    pub unsafe fn from_raw(buffer: *mut AHardwareBuffer) -> Self {
        Self { buffer }
    }

    /// Returns the raw pointer. Does not release ownership.
    pub fn as_raw(&self) -> *mut AHardwareBuffer {
        self.buffer
    }

    /// Acquires a reference to the underlying AHardwareBuffer.
    ///
    /// # Safety
    /// The buffer must still be valid (not yet released).
    pub unsafe fn acquire(&self) {
        ndk_sys::AHardwareBuffer_acquire(self.buffer);
    }

    /// Releases a reference to the underlying AHardwareBuffer.
    ///
    /// # Safety
    /// Must not be called more times than acquire() was called.
    pub unsafe fn release(&self) {
        ndk_sys::AHardwareBuffer_release(self.buffer);
    }

    /// Describes the AHardwareBuffer.
    ///
    /// # Safety
    /// The buffer must be valid.
    pub unsafe fn describe(&self) -> AHardwareBuffer_Desc {
        let mut desc = std::mem::zeroed();
        ndk_sys::AHardwareBuffer_describe(self.buffer, &mut desc);
        desc
    }
}

impl Clone for AHardwareBufferHandle {
    fn clone(&self) -> Self {
        unsafe {
            self.acquire();
        }
        Self { buffer: self.buffer }
    }
}

impl Drop for AHardwareBufferHandle {
    fn drop(&mut self) {
        unsafe {
            self.release();
        }
    }
}

/// Trait for providers that supply AHardwareBuffer for zero-copy texture integration.
///
/// This trait is the zero-copy analogue of `PayloadProvider<BoxedPixelData>`.
/// Instead of copying pixel data, the provider returns an `AHardwareBufferHandle`
/// that can be used directly with EGL Image or other GPU-backed surfaces.
///
/// # Thread Safety
/// Implementations must be `Send + Sync`. The `get_buffer()` method may be
/// called from the Platform Thread (during `mark_frame_available()`) or
/// from a Worker Thread (during preparation).
pub trait AHardwareBufferProvider: Send + Sync {
    /// Returns the current AHardwareBuffer.
    ///
    /// The returned buffer must be valid for the duration of its use by
    /// the texture system. Ownership is not transferred; the texture
    /// system will clone the handle as needed.
    fn get_buffer(&self) -> AHardwareBufferHandle;

    /// Returns the width of the buffer in pixels.
    fn get_width(&self) -> i32;

    /// Returns the height of the buffer in pixels.
    fn get_height(&self) -> i32;
}

/// Trait for explicit flush of pending AHardwareBuffer payload.
///
/// This trait allows normalizing Android's push model to match Darwin's
/// pull model. When implemented, `mark_frame_available()` can defer the
/// actual buffer upload until `flush_payload()` is called.
///
/// # Usage
/// ```ignore
/// // Instead of:
/// //   mark_frame_available() → immediately locks ANativeWindow and copies
///
/// // Use:
/// //   mark_frame_available() → sets flag only
/// //   flush_payload() → explicit upload (Platform Thread)
/// ```
pub trait DeferredPayloadFlush {
    /// Explicitly flush pending payload to the surface.
    ///
    /// If not called, flush happens automatically at drop or next
    /// `mark_frame_available()`.
    ///
    /// # Thread Safety
    /// Must be called from Platform Thread.
    fn flush_payload(&self) -> Result<()>;
}

// ============================================================================

use crate::{
    log::OkLog, BoxedPixelData, PayloadPathContract, PayloadProvider, PayloadTiming,
    PixelFormat, PlatformTextureWithProvider, PlatformTextureWithoutProvider, Result,
};

// ============================================================================
// F-02: Android Payload Path Contract Implementation
// ============================================================================

impl PayloadPathContract for BoxedPixelData {
    /// Android fetches payload DURING mark_frame_available() on the caller thread
    /// (typically Platform Thread).
    fn payload_timing() -> PayloadTiming {
        PayloadTiming::PushDuringMarkAvailable
    }
}

impl PayloadPathContract for NativeWindow {
    /// NativeWindow doesn't use get_payload(); direct surface access.
    fn payload_timing() -> PayloadTiming {
        PayloadTiming::NotApplicable
    }
}

impl PayloadPathContract for Surface {
    /// Surface doesn't use get_payload(); direct surface access.
    fn payload_timing() -> PayloadTiming {
        PayloadTiming::NotApplicable
    }
}

// ============================================================================

#[derive(PartialEq, Eq, Clone, Copy)]
struct Geometry {
    width: i32,
    height: i32,
    format: i32,
}

pub struct PlatformTexture<Type> {
    id: i64,
    texture_entry: GlobalRef,
    surface: GlobalRef,
    native_window: *mut ANativeWindow,
    last_geometry: RefCell<Option<Geometry>>,
    pixel_data_provider: Option<Arc<dyn PayloadProvider<BoxedPixelData>>>,
    _phantom: PhantomData<Type>,
}

pub(crate) const PIXEL_DATA_FORMAT: PixelFormat = PixelFormat::RGBA;

impl<Type> PlatformTexture<Type> {
    pub fn id(&self) -> i64 {
        self.id
    }

    fn new(
        engine_handle: i64,
        pixel_buffer_provider: Option<Arc<dyn PayloadProvider<BoxedPixelData>>>,
    ) -> Result<Self> {
        let java_vm = EngineContext::get_java_vm().map_err(|e| {
            match e {
                irondash_engine_context::Error::InvalidThread => Error::invalid_thread(),
                irondash_engine_context::Error::InvalidHandle => Error::invalid_handle(),
                _ => Error::from(e),
            }
        })?;
        let mut env = java_vm.attach_current_thread().map_err(Error::JNIError)?;
        let engine_context = EngineContext::get().map_err(|e| Error::from(e))?;
        let texture_registry = engine_context.get_texture_registry(engine_handle).map_err(|e| {
            match e {
                irondash_engine_context::Error::InvalidThread => Error::invalid_thread(),
                irondash_engine_context::Error::InvalidHandle => Error::invalid_handle(),
                _ => Error::from(e),
            }
        })?;

        // F-01: Handle potential registration failure with proper error taxonomy
        let texture_entry = env
            .call_method(
                texture_registry.as_obj(),
                "createSurfaceTexture",
                "()Lio/flutter/view/TextureRegistry$SurfaceTextureEntry;",
                &[],
            )
            .map_err(|e| {
                // JNI error during registration - treat as native registration failure
                Error::native_registration_failed(Some(format!(
                    "createSurfaceTexture failed: {:?}",
                    e
                )))
            })?
            .l()
            .map_err(|e| {
                Error::native_registration_failed(Some(format!(
                    "createSurfaceTexture returned null: {:?}",
                    e
                )))
            })?;

        let surface_texture = env
            .call_method(
                &texture_entry,
                "surfaceTexture",
                "()Landroid/graphics/SurfaceTexture;",
                &[],
            )?
            .l()?;
        let surface_class = env.find_class("android/view/Surface")?;

        env.push_local_frame(16)?;

        let surface = env.new_object(
            surface_class,
            "(Landroid/graphics/SurfaceTexture;)V",
            &[(&surface_texture).into()],
        )?;

        let native_window =
            unsafe { ANativeWindow_fromSurface(env.get_native_interface(), surface.as_raw()) };

        let id = env.call_method(&texture_entry, "id", "()J", &[])?.j()?;

        let res = Self {
            id,
            texture_entry: env.new_global_ref(texture_entry).map_err(Error::JNIError)?,
            surface: env.new_global_ref(surface).map_err(Error::JNIError)?,
            native_window,
            last_geometry: RefCell::new(None),
            pixel_data_provider: pixel_buffer_provider,
            _phantom: PhantomData {},
        };
        unsafe {
            env.pop_local_frame(&JObject::null())?;
        }
        Ok(res)
    }

    // ========================================================================
    // F-05: Engine Teardown Helper - Silent destroy for teardown
    // ========================================================================
    fn destroy(&mut self) -> Result<()> {
        let java_vm = EngineContext::get_java_vm().map_err(|e| Error::from(e))?;

        // F-05: Check if we're on a valid thread and engine exists
        match java_vm.attach_current_thread() {
            Ok(mut env) => {
                // Try to release texture entry; ignore errors during teardown
                let _ = env.call_method(self.texture_entry.as_obj(), "release", "()V", &[]);
                unsafe {
                    ANativeWindow_release(self.native_window);
                }
                Ok(())
            }
            Err(_) => {
                // Thread attachment failed; likely during teardown
                // Silent no-op to avoid noisy failures
                Ok(())
            }
        }
    }

    /// Explicit unregister with error reporting (for non-teardown scenarios)
    pub fn unregister(&mut self) -> Result<()> {
        let java_vm = EngineContext::get_java_vm().map_err(|e| Error::from(e))?;
        let mut env = java_vm.attach_current_thread().map_err(Error::JNIError)?;
        env.call_method(self.texture_entry.as_obj(), "release", "()V", &[])?;
        unsafe {
            ANativeWindow_release(self.native_window);
        }
        Ok(())
    }

    pub fn mark_frame_available(&self) -> Result<()> {
        if let Some(provider) = self.pixel_data_provider.as_ref() {
            let payload = provider.get_payload();
            let payload = payload.get();
            let geometry = Geometry {
                width: payload.width,
                height: payload.height,
                format: AHardwareBuffer_Format::AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM.0 as i32,
            };
            let mut last_geometry = self.last_geometry.borrow_mut();
            if *last_geometry != Some(geometry) {
                unsafe {
                    ANativeWindow_setBuffersGeometry(
                        self.native_window,
                        geometry.width,
                        geometry.height,
                        geometry.format,
                    );
                }
                last_geometry.replace(geometry);
            }
            let mut buf: ANativeWindow_Buffer = unsafe { std::mem::zeroed() };

            let data = unsafe {
                ANativeWindow_lock(self.native_window, &mut buf as *mut _, std::ptr::null_mut());
                slice::from_raw_parts_mut(
                    buf.bits as *mut u8,
                    (buf.height * buf.stride * 4) as usize,
                )
            };

            if buf.stride == buf.width {
                assert!(buf.stride * buf.height * 4 == payload.data.len() as i32);
                data.copy_from_slice(payload.data);
            } else {
                let src_stride = payload.width * 4;
                let dst_stride = buf.stride * 4;
                let min_stride = std::cmp::min(src_stride, dst_stride);
                let mut src_offset: usize = 0;
                let mut dst_offset: usize = 0;
                for _ in 0..payload.height {
                    let src_slice = &payload.data[src_offset..src_offset + min_stride as usize];
                    let dst_slice = &mut data[dst_offset..dst_offset + min_stride as usize];
                    dst_slice.copy_from_slice(src_slice);
                    src_offset += src_stride as usize;
                    dst_offset += dst_stride as usize;
                }
            }

            unsafe { ANativeWindow_unlockAndPost(self.native_window) };
        }
        Ok(())
    }
}

impl<Type> Drop for PlatformTexture<Type> {
    fn drop(&mut self) {
        self.destroy().ok_log();
    }
}

impl PlatformTextureWithProvider for BoxedPixelData {
    fn create_texture(
        engine_handle: i64,
        payload_provider: Arc<dyn PayloadProvider<Self>>,
    ) -> Result<PlatformTexture<BoxedPixelData>> {
        PlatformTexture::new(engine_handle, Some(payload_provider))
    }
}

pub struct NativeWindow {
    native_window: *mut ANativeWindow,
}

impl NativeWindow {
    fn new(native_window: *mut ANativeWindow) -> Self {
        unsafe { ANativeWindow_acquire(native_window) };
        Self { native_window }
    }

    pub fn get_native_window(&self) -> *mut ANativeWindow {
        self.native_window
    }
}

impl Clone for NativeWindow {
    fn clone(&self) -> Self {
        Self::new(self.native_window)
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        unsafe {
            ANativeWindow_release(self.native_window);
        }
    }
}

impl PlatformTextureWithoutProvider for NativeWindow {
    fn create_texture(engine_handle: i64) -> Result<PlatformTexture<NativeWindow>> {
        PlatformTexture::new(engine_handle, None)
    }

    fn get(texture: &PlatformTexture<Self>) -> Self {
        Self::new(texture.native_window)
    }
}

pub struct Surface(pub GlobalRef);

impl PlatformTextureWithoutProvider for Surface {
    fn create_texture(engine_handle: i64) -> Result<PlatformTexture<Surface>> {
        PlatformTexture::new(engine_handle, None)
    }

    fn get(texture: &PlatformTexture<Self>) -> Self {
        Self(texture.surface.clone())
    }
}
