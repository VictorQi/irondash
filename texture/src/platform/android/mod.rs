use std::{
    cell::RefCell,
    cmp::min,
    marker::PhantomData,
    os::fd::OwnedFd,
    slice,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
};

use irondash_engine_context::EngineContext;
use jni::{
    objects::{GlobalRef, JClass, JObject, JString},
    sys::{jboolean, jint},
    JNIEnv, NativeMethod,
};
use ndk_sys::{
    AHardwareBuffer, AHardwareBuffer_Format, AHardwareBuffer_Desc,
    AHardwareBuffer_UsageFlags,
    ANativeWindow, ANativeWindow_Buffer, ANativeWindow_acquire,
    ANativeWindow_fromSurface, ANativeWindow_lock, ANativeWindow_release,
    ANativeWindow_setBuffersGeometry, ANativeWindow_unlockAndPost,
};

unsafe extern "C" {
    fn AHardwareBuffer_toHardwareBuffer(
        env: *mut jni::sys::JNIEnv,
        hardware_buffer: *mut AHardwareBuffer,
    ) -> jni::sys::jobject;
}

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
    "Android hardware-buffer consumer import backend is unavailable on this runtime";
const IMPORT_TEXTURE_HELPER_CLASS: &str =
    "dev.irondash.engine_context.HardwareBufferImportTexture";
const SURFACE_CLASS: &str = "android/view/Surface";
const SURFACE_NATIVE_OBJECT_FIELD: &str = "mNativeObject";
const SURFACE_NATIVE_ATTACH_AND_QUEUE_METHOD: &str =
    "nativeAttachAndQueueBufferWithColorSpace";
const SURFACE_NATIVE_ATTACH_AND_QUEUE_SIG: &str =
    "(JLandroid/hardware/HardwareBuffer;I)I";

struct ImportedHardwareBufferReleaseCallback {
    provider: Arc<dyn AHardwareBufferFrameProvider>,
}

struct ImportedHardwareBufferTextureState {
    provider: Arc<dyn AHardwareBufferFrameProvider>,
    last_generation: AtomicU64,
    last_size: RefCell<Option<(i32, i32)>>,
    _release_callback: Box<ImportedHardwareBufferReleaseCallback>,
}

impl ImportedHardwareBufferTexture {
    /// Returns whether this fork build can perform real Android consumer import.
    pub fn import_backend_available() -> bool {
        let Ok(java_vm) = EngineContext::get_java_vm() else {
            return false;
        };
        let Ok(mut env) = java_vm.attach_current_thread() else {
            return false;
        };

        match import_texture_backend_supported(&mut env) {
            Ok(supported) => supported,
            Err(_) => {
                clear_pending_exception(&mut env);
                false
            }
        }
    }

    /// Returns the current reason why final-path consumer import is unavailable.
    pub fn unavailable_reason() -> &'static str {
        IMPORT_BACKEND_UNAVAILABLE_REASON
    }
}

static IMPORT_TEXTURE_NATIVE_REGISTRATION: OnceLock<std::result::Result<(), String>> =
    OnceLock::new();

fn load_engine_context_class<'a>(env: &mut JNIEnv<'a>, class_name: &str) -> Result<JClass<'a>> {
    let class_loader = EngineContext::get_class_loader().map_err(Error::from)?;
    let class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[(&env.new_string(class_name)?).into()],
        )?
        .l()?;
    Ok(class.into())
}

fn ensure_import_texture_helper_natives(env: &mut JNIEnv) -> Result<()> {
    let registration_result = IMPORT_TEXTURE_NATIVE_REGISTRATION.get_or_init(|| {
        let class =
            load_engine_context_class(env, IMPORT_TEXTURE_HELPER_CLASS).map_err(|err| err.to_string())?;
        let methods = [
            NativeMethod {
                name: "nativeOnImageReleased".into(),
                sig: "(JJ)V".into(),
                fn_ptr: imported_hardware_buffer_texture_on_image_released as *mut _,
            },
            NativeMethod {
                name: "nativeIsSurfaceAttachAndQueueSupported".into(),
                sig: "()Z".into(),
                fn_ptr: imported_hardware_buffer_texture_native_is_surface_attach_and_queue_supported
                    as *mut _,
            },
            NativeMethod {
                name: "nativeAttachAndQueueBufferToSurface".into(),
                sig: "(Landroid/view/Surface;Landroid/hardware/HardwareBuffer;I)V".into(),
                fn_ptr: imported_hardware_buffer_texture_native_attach_and_queue_buffer_to_surface
                    as *mut _,
            },
        ];

        env.register_native_methods(class, &methods)
            .map_err(|err| err.to_string())
    });

    match registration_result {
        Ok(()) => Ok(()),
        Err(reason) => {
            clear_pending_exception(env);
            Err(Error::native_registration_failed(Some(reason.clone())))
        }
    }
}

fn import_texture_backend_supported(env: &mut JNIEnv) -> Result<bool> {
    ensure_import_texture_helper_natives(env)?;
    let class = match load_engine_context_class(env, IMPORT_TEXTURE_HELPER_CLASS) {
        Ok(class) => class,
        Err(error) => {
            clear_pending_exception(env);
            return Err(error);
        }
    };
    match env.call_static_method(class, "isSupported", "()Z", &[]) {
        Ok(value) => match value.z() {
            Ok(supported) => Ok(supported),
            Err(error) => {
                clear_pending_exception(env);
                Err(Error::from(error))
            }
        },
        Err(error) => {
            clear_pending_exception(env);
            Err(Error::from(error))
        }
    }
}

fn import_texture_backend_unavailable_reason(env: &mut JNIEnv) -> Result<String> {
    ensure_import_texture_helper_natives(env)?;
    let class = match load_engine_context_class(env, IMPORT_TEXTURE_HELPER_CLASS) {
        Ok(class) => class,
        Err(error) => {
            clear_pending_exception(env);
            return Err(error);
        }
    };
    let reason = match env.call_static_method(class, "unavailableReason", "()Ljava/lang/String;", &[]) {
        Ok(value) => match value.l() {
            Ok(reason) => reason,
            Err(error) => {
                clear_pending_exception(env);
                return Err(Error::from(error));
            }
        },
        Err(error) => {
            clear_pending_exception(env);
            return Err(Error::from(error));
        }
    };
    if env.is_same_object(&reason, JObject::null())? {
        return Ok(IMPORT_BACKEND_UNAVAILABLE_REASON.to_string());
    }

    let reason: JString = reason.into();
    let reason = env.get_string(&reason)?;
    Ok(reason.into())
}

#[allow(non_snake_case)]
extern "system" fn imported_hardware_buffer_texture_on_image_released(
    _env: JNIEnv,
    _class: JClass,
    native_handle: i64,
    release_token: i64,
) {
    if native_handle == 0 {
        return;
    }

    let Ok(release_token) = u64::try_from(release_token) else {
        return;
    };

    let callback = unsafe { &*(native_handle as *const ImportedHardwareBufferReleaseCallback) };
    callback
        .provider
        .release_frame(HardwareBufferFrameRelease {
            release_token,
            release_fence_fd: None,
        })
        .ok_log();
}

fn surface_attach_and_queue_supported(env: &mut JNIEnv) -> Result<bool> {
    let surface_class = env.find_class(SURFACE_CLASS)?;
    env.get_field_id(&surface_class, SURFACE_NATIVE_OBJECT_FIELD, "J")?;
    env.get_static_method_id(
        &surface_class,
        SURFACE_NATIVE_ATTACH_AND_QUEUE_METHOD,
        SURFACE_NATIVE_ATTACH_AND_QUEUE_SIG,
    )?;
    Ok(true)
}

fn attach_and_queue_buffer_to_surface(
    env: &mut JNIEnv,
    surface: &JObject,
    hardware_buffer: &JObject,
    color_space_id: jint,
) -> Result<()> {
    let native_object = env.get_field(surface, SURFACE_NATIVE_OBJECT_FIELD, "J")?.j()?;
    if native_object == 0 {
        return Err(Error::texture_operation_failed(
            "Surface.mNativeObject was null while importing HardwareBuffer",
        ));
    }

    let surface_class = env.find_class(SURFACE_CLASS)?;
    let result = env
        .call_static_method(
            surface_class,
            SURFACE_NATIVE_ATTACH_AND_QUEUE_METHOD,
            SURFACE_NATIVE_ATTACH_AND_QUEUE_SIG,
            &[
                native_object.into(),
                hardware_buffer.into(),
                color_space_id.into(),
            ],
        )?
        .i()?;

    if result != 0 {
        return Err(Error::texture_operation_failed(format!(
            "Surface.nativeAttachAndQueueBufferWithColorSpace failed with error code {result}",
        )));
    }

    Ok(())
}

fn clear_pending_exception(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        env.exception_clear().ok();
    }
}

#[allow(non_snake_case)]
extern "system" fn imported_hardware_buffer_texture_native_is_surface_attach_and_queue_supported(
    mut env: JNIEnv,
    _class: JClass,
) -> jboolean {
    match surface_attach_and_queue_supported(&mut env) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(_) => {
            clear_pending_exception(&mut env);
            0
        }
    }
}

#[no_mangle]
#[allow(non_snake_case)]
extern "system" fn Java_dev_irondash_engine_1context_HardwareBufferImportTexture_nativeIsSurfaceAttachAndQueueSupported(
    env: JNIEnv,
    class: JClass,
) -> jboolean {
    imported_hardware_buffer_texture_native_is_surface_attach_and_queue_supported(env, class)
}

#[allow(non_snake_case)]
extern "system" fn imported_hardware_buffer_texture_native_attach_and_queue_buffer_to_surface(
    mut env: JNIEnv,
    _class: JClass,
    surface: JObject,
    hardware_buffer: JObject,
    color_space_id: jint,
) {
    if let Err(error) = attach_and_queue_buffer_to_surface(
        &mut env,
        &surface,
        &hardware_buffer,
        color_space_id,
    ) {
        clear_pending_exception(&mut env);
        let _ = env.throw_new("java/lang/RuntimeException", error.to_string());
    }
}

#[no_mangle]
#[allow(non_snake_case)]
extern "system" fn Java_dev_irondash_engine_1context_HardwareBufferImportTexture_nativeAttachAndQueueBufferToSurface(
    env: JNIEnv,
    class: JClass,
    surface: JObject,
    hardware_buffer: JObject,
    color_space_id: jint,
) {
    imported_hardware_buffer_texture_native_attach_and_queue_buffer_to_surface(
        env,
        class,
        surface,
        hardware_buffer,
        color_space_id,
    )
}

#[no_mangle]
#[allow(non_snake_case)]
extern "system" fn Java_dev_irondash_engine_1context_HardwareBufferImportTexture_nativeOnImageReleased(
    env: JNIEnv,
    class: JClass,
    native_handle: i64,
    release_token: i64,
) {
    imported_hardware_buffer_texture_on_image_released(env, class, native_handle, release_token)
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
    surface: Option<GlobalRef>,
    native_window: Option<*mut ANativeWindow>,
    last_geometry: RefCell<Option<Geometry>>,
    pixel_data_provider: Option<Arc<dyn PayloadProvider<BoxedPixelData>>>,
    hardware_buffer_source: Option<AndroidHardwareBufferTextureSource>,
    import_state: Option<ImportedHardwareBufferTextureState>,
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
            surface: Some(env.new_global_ref(surface).map_err(Error::JNIError)?),
            native_window: Some(native_window),
            last_geometry: RefCell::new(None),
            pixel_data_provider: pixel_buffer_provider,
            hardware_buffer_source,
            import_state: None,
            _phantom: PhantomData {},
        };
        unsafe {
            env.pop_local_frame(&JObject::null())?;
        }
        Ok(res)
    }

    fn new_imported(
        engine_handle: i64,
        frame_provider: Arc<dyn AHardwareBufferFrameProvider>,
    ) -> Result<Self> {
        let java_vm = EngineContext::get_java_vm().map_err(|e| match e {
            irondash_engine_context::Error::InvalidThread => Error::invalid_thread(),
            irondash_engine_context::Error::InvalidHandle => Error::invalid_handle(),
            _ => Error::from(e),
        })?;
        let mut env = java_vm.attach_current_thread().map_err(Error::JNIError)?;
        clear_pending_exception(&mut env);
        let engine_context = EngineContext::get().map_err(Error::from)?;
        let texture_registry = engine_context.get_texture_registry(engine_handle).map_err(|e| {
            match e {
                irondash_engine_context::Error::InvalidThread => Error::invalid_thread(),
                irondash_engine_context::Error::InvalidHandle => Error::invalid_handle(),
                _ => Error::from(e),
            }
        })?;

        ensure_import_texture_helper_natives(&mut env)?;
        if !import_texture_backend_supported(&mut env)? {
            return Err(Error::native_registration_failed(Some(
                import_texture_backend_unavailable_reason(&mut env)
                    .unwrap_or_else(|_| IMPORT_BACKEND_UNAVAILABLE_REASON.to_string()),
            )));
        }

        log::debug!(
            "irondash_texture: registering Android HardwareBufferImport texture for engine_handle={engine_handle}"
        );

        let release_callback = Box::new(ImportedHardwareBufferReleaseCallback {
            provider: frame_provider.clone(),
        });
        let native_handle = release_callback.as_ref() as *const _ as i64;

        env.push_local_frame(32)?;
        let helper_result = (|| -> Result<(GlobalRef, i64)> {
            let helper_class = load_engine_context_class(&mut env, IMPORT_TEXTURE_HELPER_CLASS)?;
            let helper = env.new_object(
                helper_class,
                "(Lio/flutter/view/TextureRegistry;J)V",
                &[texture_registry.as_obj().into(), native_handle.into()],
            )?;
            let id = env.call_method(&helper, "id", "()J", &[])?.j()?;
            let helper_ref = env.new_global_ref(helper).map_err(Error::JNIError)?;
            Ok((helper_ref, id))
        })();
        unsafe {
            env.pop_local_frame(&JObject::null())?;
        }
        let (helper_ref, id) = helper_result?;

        Ok(Self {
            id,
            texture_entry: helper_ref,
            surface: None,
            native_window: None,
            last_geometry: RefCell::new(None),
            pixel_data_provider: None,
            hardware_buffer_source: None,
            import_state: Some(ImportedHardwareBufferTextureState {
                provider: frame_provider,
                last_generation: AtomicU64::new(0),
                last_size: RefCell::new(None),
                _release_callback: release_callback,
            }),
            _phantom: PhantomData {},
        })
    }

    // ========================================================================
    // F-05: Engine Teardown Helper - Silent destroy for teardown
    // ========================================================================
    fn destroy(&mut self) -> Result<()> {
        let java_vm = EngineContext::get_java_vm().map_err(|e| Error::from(e))?;

        // F-05: Check if we're on a valid thread and engine exists
        match java_vm.attach_current_thread() {
            Ok(mut env) => {
                if self.import_state.is_some() {
                    let _ = env.call_method(self.texture_entry.as_obj(), "close", "()V", &[]);
                } else {
                    // Try to release texture entry; ignore errors during teardown
                    let _ = env.call_method(self.texture_entry.as_obj(), "release", "()V", &[]);
                    if let Some(native_window) = self.native_window.take() {
                        unsafe {
                            ANativeWindow_release(native_window);
                        }
                    }
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
        if self.import_state.is_some() {
            env.call_method(self.texture_entry.as_obj(), "close", "()V", &[])?;
        } else {
            env.call_method(self.texture_entry.as_obj(), "release", "()V", &[])?;
            if let Some(native_window) = self.native_window.take() {
                unsafe {
                    ANativeWindow_release(native_window);
                }
            }
        }
        Ok(())
    }

    pub fn mark_frame_available(&self) -> Result<()> {
        if let Some(import_state) = self.import_state.as_ref() {
            return self.queue_imported_hardware_buffer_frame(import_state);
        }

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
            let native_window = self.native_window.ok_or_else(|| {
                Error::texture_operation_failed(
                    "Android pixel-data upload requires a native-window-backed texture",
                )
            })?;
            let mut buf: ANativeWindow_Buffer = unsafe { std::mem::zeroed() };

            let data = unsafe {
                ANativeWindow_lock(native_window, &mut buf as *mut _, std::ptr::null_mut());
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

            unsafe { ANativeWindow_unlockAndPost(native_window) };
        }
        Ok(())
    }

    fn queue_imported_hardware_buffer_frame(
        &self,
        import_state: &ImportedHardwareBufferTextureState,
    ) -> Result<()> {
        let after_generation = import_state.last_generation.load(Ordering::Acquire);
        let frame = match import_state.provider.acquire_latest_frame(after_generation)? {
            AcquireFrameOutcome::Acquired(frame) => frame,
            AcquireFrameOutcome::NoNewFrame { latest_generation } => {
                if latest_generation > after_generation {
                    import_state
                        .last_generation
                        .store(latest_generation, Ordering::Release);
                }
                return Ok(());
            }
        };

        let HardwareBufferFrame {
            buffer,
            width,
            height,
            format,
            generation,
            acquire_fence_fd,
            release_token,
        } = frame;

        drop(acquire_fence_fd);

        let result = (|| -> Result<()> {
            if format != AndroidHardwareBufferFormat::Rgba8888 {
                return Err(Error::texture_operation_failed(format!(
                    "Android hardware-buffer import currently supports only RGBA8888; got {:?}",
                    format,
                )));
            }

            log::debug!(
                "irondash_texture: queueing imported hardware buffer frame generation={} size={}x{} release_token={}",
                generation,
                width,
                height,
                release_token,
            );

            let java_vm = EngineContext::get_java_vm().map_err(Error::from)?;
            let mut env = java_vm.attach_current_thread().map_err(Error::JNIError)?;
            let width = width.max(1);
            let height = height.max(1);
            if *import_state.last_size.borrow() != Some((width, height)) {
                env.call_method(
                    self.texture_entry.as_obj(),
                    "setSize",
                    "(II)V",
                    &[width.into(), height.into()],
                )?;
                import_state.last_size.replace(Some((width, height)));
            }

            env.push_local_frame(16)?;
            let queue_result = (|| -> Result<()> {
                let hardware_buffer_obj = unsafe {
                    AHardwareBuffer_toHardwareBuffer(env.get_native_interface(), buffer.as_raw())
                };
                if hardware_buffer_obj.is_null() {
                    return Err(Error::texture_operation_failed(
                        "AHardwareBuffer_toHardwareBuffer returned null",
                    ));
                }
                let hardware_buffer_obj = unsafe { JObject::from_raw(hardware_buffer_obj) };
                env.call_method(
                    self.texture_entry.as_obj(),
                    "queueHardwareBuffer",
                    "(Landroid/hardware/HardwareBuffer;J)V",
                    &[(&hardware_buffer_obj).into(), (release_token as i64).into()],
                )?;
                Ok(())
            })();
            unsafe {
                env.pop_local_frame(&JObject::null())?;
            }
            queue_result
        })();

        match result {
            Ok(()) => {
                import_state
                    .last_generation
                    .store(generation, Ordering::Release);
                Ok(())
            }
            Err(error) => {
                import_state
                    .provider
                    .release_frame(HardwareBufferFrameRelease {
                        release_token,
                        release_fence_fd: None,
                    })
                    .ok_log();
                Err(error)
            }
        }
    }

    fn ensure_window_geometry(&self, geometry: Geometry) -> Result<()> {
        let mut last_geometry = self.last_geometry.borrow_mut();
        if *last_geometry == Some(geometry) {
            return Ok(());
        }

        let native_window = self.native_window.ok_or_else(|| {
            Error::texture_operation_failed(
                "Android native-window geometry update requires a native-window-backed texture",
            )
        })?;

        let status = unsafe {
            ANativeWindow_setBuffersGeometry(
                native_window,
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
        let native_window = self.native_window.ok_or_else(|| {
            Error::texture_operation_failed(
                "Android hardware-buffer flush requires a native-window-backed texture",
            )
        })?;
        let window_lock_status = unsafe {
            ANativeWindow_lock(native_window, &mut window_buffer, std::ptr::null_mut())
        };
        if window_lock_status != 0 {
            let _ = unlock_hardware_buffer(buffer.as_raw());
            return Err(Error::texture_operation_failed(format!(
                "ANativeWindow_lock failed with status {window_lock_status}"
            )));
        }
        if window_buffer.bits.is_null() {
            let _ = unlock_hardware_buffer(buffer.as_raw());
            let _ = unsafe { ANativeWindow_unlockAndPost(native_window) };
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
        let window_unlock_status = unsafe { ANativeWindow_unlockAndPost(native_window) };

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
        engine_handle: i64,
        provider: Arc<dyn AHardwareBufferFrameProvider>,
    ) -> Result<PlatformTexture<ImportedHardwareBufferTexture>> {
        PlatformTexture::new_imported(engine_handle, provider)
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
        Self::new(
            texture
                .native_window
                .expect("native-window-backed texture must retain ANativeWindow"),
        )
    }
}

pub struct Surface(pub GlobalRef);

impl PlatformTextureWithoutProvider for Surface {
    fn create_texture(engine_handle: i64) -> Result<PlatformTexture<Surface>> {
        PlatformTexture::new(engine_handle, None, None)
    }

    fn get(texture: &PlatformTexture<Self>) -> Self {
        Self(
            texture
                .surface
                .as_ref()
                .expect("surface-backed texture must retain Surface")
                .clone(),
        )
    }
}
