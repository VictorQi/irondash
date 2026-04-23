use std::{cell::RefCell, cmp::min, marker::PhantomData, os::fd::OwnedFd, slice, sync::Arc};

use irondash_engine_context::EngineContext;
use jni::objects::{GlobalRef, JObject};
use ndk_sys::{
    AHardwareBuffer, AHardwareBuffer_Format, AHardwareBuffer_Desc,
    AHardwareBuffer_UsageFlags,
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

    /// Creates a new handle from a borrowed raw pointer by first acquiring an
    /// extra AHardwareBuffer reference.
    ///
    /// # Safety
    /// The pointer must remain valid for the duration of the acquire call.
    pub unsafe fn from_borrowed_raw(buffer: *mut AHardwareBuffer) -> Self {
        ndk_sys::AHardwareBuffer_acquire(buffer);
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

/// Canonical Android hardware-buffer format understood by the import path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AndroidHardwareBufferFormat {
    /// `AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM`
    Rgba8888,
}

/// Per-frame lease returned by the Android hardware-buffer import provider.
pub struct HardwareBufferFrame {
    /// Acquired hardware-buffer handle.
    pub buffer: AHardwareBufferHandle,
    /// Visible width in pixels.
    pub width: i32,
    /// Visible height in pixels.
    pub height: i32,
    /// Producer-declared pixel format.
    pub format: AndroidHardwareBufferFormat,
    /// Monotonic generation for the shared source.
    pub generation: u64,
    /// Optional acquire fence transferred to the consumer.
    pub acquire_fence_fd: Option<OwnedFd>,
    /// Provider-unique release token for this lease.
    pub release_token: u64,
}

/// Completion payload returned to the frame provider after a lease retires.
pub struct HardwareBufferFrameRelease {
    /// Provider-issued token that identifies the frame being released.
    pub release_token: u64,
    /// Optional release fence transferred back to the producer.
    pub release_fence_fd: Option<OwnedFd>,
}

/// Outcome returned by `AHardwareBufferFrameProvider::acquire_latest_frame()`.
pub enum AcquireFrameOutcome {
    /// A concrete frame lease was acquired.
    Acquired(HardwareBufferFrame),
    /// No frame newer than `after_generation` is currently available.
    NoNewFrame {
        /// Latest generation currently known to the provider.
        latest_generation: u64,
    },
}

/// Provider contract for the final Android hardware-buffer import path.
pub trait AHardwareBufferFrameProvider: Send + Sync {
    /// Acquires the newest frame lease after the provided generation.
    fn acquire_latest_frame(&self, after_generation: u64) -> Result<AcquireFrameOutcome>;

    /// Releases a previously acquired frame lease.
    fn release_frame(&self, release: HardwareBufferFrameRelease) -> Result<()>;
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

/// Identifies how a zero-copy source reaches the Android hardware buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AndroidHardwareBufferSourceKind {
    /// Source owns a direct AHardwareBuffer handle.
    AHardwareBuffer,
    /// Source reaches the hardware buffer through an EGL image wrapper.
    EglImageBackingBuffer,
}

/// Explicit Android zero-copy seam input for texture registration.
#[derive(Clone)]
pub struct AndroidHardwareBufferTextureSource {
    provider: Arc<dyn AHardwareBufferProvider>,
    source_kind: AndroidHardwareBufferSourceKind,
    width: i32,
    height: i32,
}

impl AndroidHardwareBufferTextureSource {
    /// Creates a new hardware-buffer-backed registration source.
    pub fn new(
        provider: Arc<dyn AHardwareBufferProvider>,
        source_kind: AndroidHardwareBufferSourceKind,
        width: i32,
        height: i32,
    ) -> Self {
        Self {
            provider,
            source_kind,
            width,
            height,
        }
    }

    /// Returns the retained provider.
    pub fn provider(&self) -> &Arc<dyn AHardwareBufferProvider> {
        &self.provider
    }

    /// Returns how this source maps to an AHardwareBuffer.
    pub fn source_kind(&self) -> AndroidHardwareBufferSourceKind {
        self.source_kind
    }

    /// Returns the visible width in pixels.
    pub fn width(&self) -> i32 {
        self.width
    }

    /// Returns the visible height in pixels.
    pub fn height(&self) -> i32 {
        self.height
    }
}

/// Marker type for the final Android hardware-buffer import path.
pub struct ImportedHardwareBufferTexture;

const IMPORT_BACKEND_UNAVAILABLE_REASON: &str =
    "Android hardware-buffer consumer import backend is not implemented in the irondash fork; only the SurfaceTexture/ANativeWindow seam exists";

impl ImportedHardwareBufferTexture {
    /// Returns whether this fork build can perform real Android consumer import.
    pub fn import_backend_available() -> bool {
        false
    }

    /// Returns the current reason why final-path consumer import is unavailable.
    pub fn unavailable_reason() -> &'static str {
        IMPORT_BACKEND_UNAVAILABLE_REASON
    }
}

// ============================================================================

use crate::{
    log::OkLog, BoxedPixelData, Error, PayloadPathContract, PayloadProvider, PayloadTiming,
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

impl PayloadPathContract for ImportedHardwareBufferTexture {
    /// Imported hardware-buffer textures do not fetch payload through `get_payload()`.
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

const BYTES_PER_PIXEL: usize = 4;

pub struct PlatformTexture<Type> {
    id: i64,
    texture_entry: GlobalRef,
    surface: GlobalRef,
    native_window: *mut ANativeWindow,
    last_geometry: RefCell<Option<Geometry>>,
    pixel_data_provider: Option<Arc<dyn PayloadProvider<BoxedPixelData>>>,
    hardware_buffer_source: Option<AndroidHardwareBufferTextureSource>,
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
        hardware_buffer_source: Option<AndroidHardwareBufferTextureSource>,
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
            hardware_buffer_source,
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
        if let Some(source) = self.hardware_buffer_source.as_ref() {
            self.flush_hardware_buffer_source(source)?;
            return Ok(());
        }

        if let Some(provider) = self.pixel_data_provider.as_ref() {
            let payload = provider.get_payload();
            let payload = payload.get();
            let geometry = Geometry {
                width: payload.width,
                height: payload.height,
                format: AHardwareBuffer_Format::AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM.0 as i32,
            };
            self.ensure_window_geometry(geometry)?;
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

    fn ensure_window_geometry(&self, geometry: Geometry) -> Result<()> {
        let mut last_geometry = self.last_geometry.borrow_mut();
        if *last_geometry == Some(geometry) {
            return Ok(());
        }

        let status = unsafe {
            ANativeWindow_setBuffersGeometry(
                self.native_window,
                geometry.width,
                geometry.height,
                geometry.format,
            )
        };
        if status != 0 {
            return Err(Error::texture_operation_failed(format!(
                "ANativeWindow_setBuffersGeometry failed with status {status}"
            )));
        }

        last_geometry.replace(geometry);
        Ok(())
    }

    fn flush_hardware_buffer_source(
        &self,
        source: &AndroidHardwareBufferTextureSource,
    ) -> Result<()> {
        let buffer = source.provider().get_buffer();
        let desc = unsafe { buffer.describe() };
        let expected_format = AHardwareBuffer_Format::AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM.0;
        if desc.format != expected_format as u32 {
            return Err(Error::texture_operation_failed(format!(
                "Android hardware-buffer flush currently supports only RGBA8888; got format {} for {:?}",
                desc.format,
                source.source_kind()
            )));
        }

        let width = if source.width() > 0 {
            source.width()
        } else {
            i32::try_from(desc.width).map_err(|_| {
                Error::texture_operation_failed("AHardwareBuffer width exceeded i32")
            })?
        };
        let height = if source.height() > 0 {
            source.height()
        } else {
            i32::try_from(desc.height).map_err(|_| {
                Error::texture_operation_failed("AHardwareBuffer height exceeded i32")
            })?
        };
        let stride_pixels = i32::try_from(desc.stride).map_err(|_| {
            Error::texture_operation_failed("AHardwareBuffer stride exceeded i32")
        })?;

        if width <= 0 || height <= 0 {
            return Err(Error::texture_operation_failed(format!(
                "Android hardware-buffer flush requires positive geometry; got {}x{}",
                width, height
            )));
        }
        if stride_pixels <= 0 || stride_pixels < width {
            return Err(Error::texture_operation_failed(format!(
                "Android hardware-buffer flush received invalid stride {} for width {}",
                stride_pixels, width
            )));
        }

        let geometry = Geometry {
            width,
            height,
            format: expected_format as i32,
        };
        self.ensure_window_geometry(geometry)?;

        let mut source_ptr = std::ptr::null_mut();
        let source_lock_status = unsafe {
            ndk_sys::AHardwareBuffer_lock(
                buffer.as_raw(),
                AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN.0 as u64,
                -1,
                std::ptr::null(),
                &mut source_ptr,
            )
        };
        if source_lock_status != 0 {
            return Err(Error::texture_operation_failed(format!(
                "AHardwareBuffer_lock failed with status {source_lock_status}"
            )));
        }
        if source_ptr.is_null() {
            let _ = unlock_hardware_buffer(buffer.as_raw());
            return Err(Error::texture_operation_failed(
                "AHardwareBuffer_lock returned a null data pointer",
            ));
        }

        let mut window_buffer = unsafe { std::mem::zeroed::<ANativeWindow_Buffer>() };
        let window_lock_status = unsafe {
            ANativeWindow_lock(self.native_window, &mut window_buffer, std::ptr::null_mut())
        };
        if window_lock_status != 0 {
            let _ = unlock_hardware_buffer(buffer.as_raw());
            return Err(Error::texture_operation_failed(format!(
                "ANativeWindow_lock failed with status {window_lock_status}"
            )));
        }
        if window_buffer.bits.is_null() {
            let _ = unlock_hardware_buffer(buffer.as_raw());
            let _ = unsafe { ANativeWindow_unlockAndPost(self.native_window) };
            return Err(Error::texture_operation_failed(
                "ANativeWindow_lock returned a null data pointer",
            ));
        }

        let copy_result = copy_hardware_buffer_rows(
            source_ptr.cast(),
            width,
            height,
            stride_pixels,
            &window_buffer,
        );
        let source_unlock_result = unlock_hardware_buffer(buffer.as_raw());
        let window_unlock_status = unsafe { ANativeWindow_unlockAndPost(self.native_window) };

        match copy_result {
            Ok(()) => {
                source_unlock_result?;
                if window_unlock_status != 0 {
                    return Err(Error::texture_operation_failed(format!(
                        "ANativeWindow_unlockAndPost failed with status {window_unlock_status}"
                    )));
                }
                Ok(())
            }
            Err(error) => {
                let _ = source_unlock_result;
                let _ = window_unlock_status;
                Err(error)
            }
        }
    }
}

fn area_bytes(height: i32, stride_pixels: i32, label: &str) -> Result<usize> {
    let height = usize::try_from(height)
        .map_err(|_| Error::texture_operation_failed(format!("{label} height was negative")))?;
    let stride_pixels = usize::try_from(stride_pixels).map_err(|_| {
        Error::texture_operation_failed(format!("{label} stride was negative"))
    })?;

    height
        .checked_mul(stride_pixels)
        .and_then(|area| area.checked_mul(BYTES_PER_PIXEL))
        .ok_or_else(|| {
            Error::texture_operation_failed(format!(
                "{label} buffer size overflowed while preparing Android hardware-buffer flush"
            ))
        })
}

fn copy_hardware_buffer_rows(
    source_ptr: *const u8,
    width: i32,
    height: i32,
    source_stride_pixels: i32,
    window_buffer: &ANativeWindow_Buffer,
) -> Result<()> {
    if window_buffer.height <= 0 || window_buffer.stride <= 0 {
        return Err(Error::texture_operation_failed(format!(
            "Android destination geometry was invalid: height={}, stride={}",
            window_buffer.height,
            window_buffer.stride
        )));
    }

    let source_byte_len = area_bytes(height, source_stride_pixels, "source")?;
    let destination_byte_len = area_bytes(window_buffer.height, window_buffer.stride, "destination")?;
    let rows = min(height, window_buffer.height);
    let visible_bytes_per_row = usize::try_from(width)
        .ok()
        .and_then(|visible_width| visible_width.checked_mul(BYTES_PER_PIXEL))
        .ok_or_else(|| Error::texture_operation_failed("visible row byte count overflowed"))?;
    let source_stride_bytes = usize::try_from(source_stride_pixels)
        .ok()
        .and_then(|stride| stride.checked_mul(BYTES_PER_PIXEL))
        .ok_or_else(|| Error::texture_operation_failed("source stride byte count overflowed"))?;
    let destination_stride_bytes = usize::try_from(window_buffer.stride)
        .ok()
        .and_then(|stride| stride.checked_mul(BYTES_PER_PIXEL))
        .ok_or_else(|| Error::texture_operation_failed("destination stride byte count overflowed"))?;
    let row_copy_bytes = min(
        visible_bytes_per_row,
        min(source_stride_bytes, destination_stride_bytes),
    );

    let source_data = unsafe { slice::from_raw_parts(source_ptr, source_byte_len) };
    let destination_data = unsafe {
        slice::from_raw_parts_mut(window_buffer.bits.cast::<u8>(), destination_byte_len)
    };

    for row in 0..usize::try_from(rows)
        .map_err(|_| Error::texture_operation_failed("destination row count was negative"))?
    {
        let source_offset = row
            .checked_mul(source_stride_bytes)
            .ok_or_else(|| Error::texture_operation_failed("source row offset overflowed"))?;
        let destination_offset = row
            .checked_mul(destination_stride_bytes)
            .ok_or_else(|| Error::texture_operation_failed("destination row offset overflowed"))?;
        let source_end = source_offset
            .checked_add(row_copy_bytes)
            .ok_or_else(|| Error::texture_operation_failed("source row copy range overflowed"))?;
        let destination_end = destination_offset
            .checked_add(row_copy_bytes)
            .ok_or_else(|| Error::texture_operation_failed("destination row copy range overflowed"))?;

        destination_data[destination_offset..destination_end]
            .copy_from_slice(&source_data[source_offset..source_end]);
    }

    Ok(())
}

fn unlock_hardware_buffer(buffer: *mut AHardwareBuffer) -> Result<()> {
    let mut release_fence = -1;
    let status = unsafe { ndk_sys::AHardwareBuffer_unlock(buffer, &mut release_fence) };
    if status != 0 {
        return Err(Error::texture_operation_failed(format!(
            "AHardwareBuffer_unlock failed with status {status}"
        )));
    }
    Ok(())
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
        PlatformTexture::new(engine_handle, Some(payload_provider), None)
    }
}

impl PlatformTexture<NativeWindow> {
    pub fn new_with_hardware_buffer_source(
        engine_handle: i64,
        source: AndroidHardwareBufferTextureSource,
    ) -> Result<PlatformTexture<NativeWindow>> {
        PlatformTexture::new(engine_handle, None, Some(source))
    }
}

impl PlatformTexture<ImportedHardwareBufferTexture> {
    pub fn new_with_hardware_buffer_frame_source(
        _engine_handle: i64,
        _provider: Arc<dyn AHardwareBufferFrameProvider>,
    ) -> Result<PlatformTexture<ImportedHardwareBufferTexture>> {
        Err(Error::native_registration_failed(Some(
            ImportedHardwareBufferTexture::unavailable_reason().into(),
        )))
    }
}

impl DeferredPayloadFlush for PlatformTexture<NativeWindow> {
    fn flush_payload(&self) -> Result<()> {
        let source = self.hardware_buffer_source.as_ref().ok_or_else(|| {
            Error::texture_operation_failed(
                "explicit hardware-buffer flush requires a registered hardware-buffer source",
            )
        })?;

        self.flush_hardware_buffer_source(source)
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
        PlatformTexture::new(engine_handle, None, None)
    }

    fn get(texture: &PlatformTexture<Self>) -> Self {
        Self::new(texture.native_window)
    }
}

pub struct Surface(pub GlobalRef);

impl PlatformTextureWithoutProvider for Surface {
    fn create_texture(engine_handle: i64) -> Result<PlatformTexture<Surface>> {
        PlatformTexture::new(engine_handle, None, None)
    }

    fn get(texture: &PlatformTexture<Self>) -> Self {
        Self(texture.surface.clone())
    }
}
