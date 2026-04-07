use std::{cell::Cell, ffi::c_void, iter::repeat_with, rc::Rc, sync::Arc, time::Duration};

use irondash_dart_ffi::DartValue;
use irondash_run_loop::RunLoop;
use irondash_texture::{BoxedPixelData, PayloadProvider, SimplePixelData, Texture};
use log::error;

#[cfg(target_os = "android")]
mod android_smoke {
    use std::ffi::c_void;
    use std::slice;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    use error_model::ResourceError;
    use ffi_api::{
        irondash_ffi_acquire_shared_texture,
        irondash_ffi_get_request_state,
        irondash_ffi_init,
        irondash_ffi_process_pending_requests,
        irondash_ffi_register_engine,
        irondash_ffi_release_texture,
        irondash_ffi_try_pop_event,
        irondash_ffi_unregister_engine,
        replace_global_api,
        FfiAcquireDecision,
        FfiApi,
        FfiEvent,
        FfiEventRecord,
        FfiEventType,
        FfiPriorityCode,
        FfiReleaseDisposition,
        FfiRequestStateSnapshot,
        SourceProvider,
    };
    use libc::close;
    use log::{debug, info, warn};
    use ndk_sys::{
        AHardwareBuffer, AHardwareBuffer_Desc, AHardwareBuffer_Format,
        AHardwareBuffer_UsageFlags,
    };
    use platform_android::{AndroidPlatformCleaner, AndroidPlatformHandle};
    use protocol::SourceId;
    use shared_source::SharedSource;

    const SMOKE_SOURCE_ID: u64 = 1001;
    const SMOKE_WIDTH: i32 = 256;
    const SMOKE_HEIGHT: i32 = 256;
    const SMOKE_BYTES_PER_PIXEL: usize = 4;

    static SMOKE_RUNTIME: OnceLock<Arc<FfiApi>> = OnceLock::new();
    static SMOKE_SESSION: OnceLock<Mutex<Option<SmokeSession>>> = OnceLock::new();

    struct SmokeSession {
        engine_handle: i64,
        request_id: u64,
        texture_id: i64,
    }

    struct AndroidSmokeSourceProvider {
        generation: AtomicU32,
    }

    impl AndroidSmokeSourceProvider {
        fn new() -> Self {
            Self {
                generation: AtomicU32::new(0),
            }
        }

        fn next_generation(&self) -> u32 {
            self.generation.fetch_add(1, Ordering::AcqRel) + 1
        }
    }

    impl SourceProvider for AndroidSmokeSourceProvider {
        fn resolve_source(&self, source_id: SourceId) -> Result<Arc<SharedSource>, ResourceError> {
            if source_id.as_u64() != SMOKE_SOURCE_ID {
                return Err(ResourceError::SourceNotFound(source_id.to_string()));
            }

            create_smoke_source(source_id, self.next_generation())
        }
    }

    pub fn acquire_texture(engine_handle: i64) -> Result<i64, String> {
        let _runtime = ensure_runtime();
        release_existing_session();

        let register = irondash_ffi_register_engine(engine_handle);
        if !register.success {
            return Err(format!(
                "register_engine failed for handle {} with error_code={}",
                engine_handle, register.error_code
            ));
        }

        let response = irondash_ffi_acquire_shared_texture(
            SMOKE_SOURCE_ID,
            engine_handle,
            FfiPriorityCode::Visible as u32,
        );
        if response.decision == FfiAcquireDecision::Rejected as u32 {
            return Err(format!(
                "acquire_shared_texture rejected with error_code={}",
                response.error_code
            ));
        }

        if response.texture_id >= 0 {
            store_session(engine_handle, response.request_id, response.texture_id);
            drain_events(response.request_id);
            info!(
                "Smoke texture ready immediately: request_id={}, texture_id={}",
                response.request_id, response.texture_id
            );
            return Ok(response.texture_id);
        }

        let processed = irondash_ffi_process_pending_requests(1);
        debug!(
            "Smoke request pumped on platform thread: request_id={}, processed={}",
            response.request_id, processed
        );

        let texture_id = find_texture_id(response.request_id).ok_or_else(|| {
            format!(
                "smoke request {} did not reach TextureReady after one pump",
                response.request_id
            )
        })?;

        store_session(engine_handle, response.request_id, texture_id);
        info!(
            "Smoke texture ready: request_id={}, texture_id={}",
            response.request_id, texture_id
        );
        Ok(texture_id)
    }

    pub fn release_texture(_engine_handle: i64) -> Result<bool, String> {
        let _runtime = ensure_runtime();
        let session = {
            let mut guard = smoke_session().lock().expect("smoke session mutex poisoned");
            guard.take()
        };

        let Some(session) = session else {
            return Ok(false);
        };

        let release = irondash_ffi_release_texture(session.request_id, session.engine_handle);
        if release.error_code != 0 {
            return Err(format!(
                "release_texture failed for request {} with error_code={}",
                session.request_id, release.error_code
            ));
        }

        let unregister = irondash_ffi_unregister_engine(session.engine_handle);
        if !unregister.success && unregister.error_code != 0 {
            return Err(format!(
                "unregister_engine failed for handle {} with error_code={}",
                session.engine_handle, unregister.error_code
            ));
        }

        drain_events(session.request_id);
        info!(
            "Smoke texture released: request_id={}, texture_id={}, release_disposition={}",
            session.request_id, session.texture_id, release.release_disposition
        );

        Ok(matches!(
            release.release_disposition,
            value if value == FfiReleaseDisposition::Released as u32
                || value == FfiReleaseDisposition::NotFound as u32
        ))
    }

    fn ensure_runtime() -> Arc<FfiApi> {
        SMOKE_RUNTIME
            .get_or_init(|| {
                irondash_ffi_init();
                let api = Arc::new(
                    FfiApi::builder()
                        .with_source_provider(Arc::new(AndroidSmokeSourceProvider::new()))
                        .with_event_callback(Arc::new(log_ffi_event))
                        .build(),
                );
                let _ = replace_global_api(api.clone());
                api
            })
            .clone()
    }

    fn smoke_session() -> &'static Mutex<Option<SmokeSession>> {
        SMOKE_SESSION.get_or_init(|| Mutex::new(None))
    }

    fn store_session(engine_handle: i64, request_id: u64, texture_id: i64) {
        let mut guard = smoke_session().lock().expect("smoke session mutex poisoned");
        *guard = Some(SmokeSession {
            engine_handle,
            request_id,
            texture_id,
        });
    }

    fn release_existing_session() {
        let session = {
            let mut guard = smoke_session().lock().expect("smoke session mutex poisoned");
            guard.take()
        };

        let Some(session) = session else {
            return;
        };

        let release = irondash_ffi_release_texture(session.request_id, session.engine_handle);
        if release.error_code != 0 {
            warn!(
                "Best-effort release before reacquire failed: request_id={}, error_code={}",
                session.request_id, release.error_code
            );
        }

        let unregister = irondash_ffi_unregister_engine(session.engine_handle);
        if !unregister.success && unregister.error_code != 0 {
            warn!(
                "Best-effort unregister before reacquire failed: engine_handle={}, error_code={}",
                session.engine_handle, unregister.error_code
            );
        }

        drain_events(session.request_id);
    }

    fn find_texture_id(request_id: u64) -> Option<i64> {
        let event_texture_id = drain_events(request_id);
        if event_texture_id.is_some() {
            return event_texture_id;
        }

        let mut state = FfiRequestStateSnapshot::default();
        if irondash_ffi_get_request_state(request_id, &mut state as *mut _) && state.texture_id >= 0 {
            return Some(state.texture_id);
        }

        None
    }

    fn drain_events(request_id: u64) -> Option<i64> {
        let mut ready_texture_id = None;

        loop {
            let mut event = FfiEventRecord::default();
            if !irondash_ffi_try_pop_event(&mut event as *mut _) {
                break;
            }

            debug!(
                "smoke ffi event: type={}, request_id={}, texture_id={}, status={}, error_code={}",
                event.event_type, event.request_id, event.texture_id, event.status, event.error_code
            );

            if event.request_id == request_id
                && event.event_type == FfiEventType::TextureReady as u32
                && event.texture_id >= 0
            {
                ready_texture_id = Some(event.texture_id);
            }
        }

        ready_texture_id
    }

    fn log_ffi_event(event: FfiEvent) {
        debug!("smoke ffi callback event: {:?}", event);
    }

    fn create_smoke_source(
        source_id: SourceId,
        generation: u32,
    ) -> Result<Arc<SharedSource>, ResourceError> {
        let cleaner = Arc::new(AndroidPlatformCleaner::new());
        let ahb_ptr = unsafe { allocate_ahardware_buffer(SMOKE_WIDTH, SMOKE_HEIGHT)? };

        if let Err(err) = unsafe { fill_ahardware_buffer(ahb_ptr, SMOKE_WIDTH, SMOKE_HEIGHT, generation) } {
            unsafe {
                ndk_sys::AHardwareBuffer_release(ahb_ptr.cast::<AHardwareBuffer>());
            }
            return Err(err);
        }

        let handle = unsafe { AndroidPlatformHandle::from_ahb(ahb_ptr, SMOKE_WIDTH, SMOKE_HEIGHT) };
        SharedSource::new_with_deferred_drop(
            source_id,
            handle.into(),
            (SMOKE_WIDTH as usize) * (SMOKE_HEIGHT as usize) * SMOKE_BYTES_PER_PIXEL,
            cleaner,
        )
        .map(Arc::new)
    }

    unsafe fn allocate_ahardware_buffer(
        width: i32,
        height: i32,
    ) -> Result<*mut c_void, ResourceError> {
        let mut desc = std::mem::zeroed::<AHardwareBuffer_Desc>();
        desc.width = width as u32;
        desc.height = height as u32;
        desc.layers = 1;
        desc.format = AHardwareBuffer_Format::AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM.0 as u32;
        desc.usage = (AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN.0
            | AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN.0
            | AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE.0) as u64;

        let mut buffer: *mut AHardwareBuffer = std::ptr::null_mut();
        let status = ndk_sys::AHardwareBuffer_allocate(&desc, &mut buffer);
        if status != 0 || buffer.is_null() {
            return Err(ResourceError::PlatformResourceFailed(format!(
                "AHardwareBuffer_allocate failed with status {}",
                status
            )));
        }

        Ok(buffer.cast())
    }

    unsafe fn fill_ahardware_buffer(
        ahb_ptr: *mut c_void,
        width: i32,
        height: i32,
        generation: u32,
    ) -> Result<(), ResourceError> {
        let ahb = ahb_ptr.cast::<AHardwareBuffer>();
        let mut desc = std::mem::zeroed::<AHardwareBuffer_Desc>();
        ndk_sys::AHardwareBuffer_describe(ahb.cast_const(), &mut desc);

        let mut data: *mut c_void = std::ptr::null_mut();
        let lock_status = ndk_sys::AHardwareBuffer_lock(
            ahb,
            AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN.0 as u64,
            -1,
            std::ptr::null(),
            &mut data,
        );
        if lock_status != 0 || data.is_null() {
            return Err(ResourceError::PlatformResourceFailed(format!(
                "AHardwareBuffer_lock failed with status {}",
                lock_status
            )));
        }

        let stride = desc.stride as usize;
        let rows = height as usize;
        let cols = width as usize;
        let byte_len = stride
            .checked_mul(rows)
            .and_then(|count| count.checked_mul(SMOKE_BYTES_PER_PIXEL))
            .ok_or_else(|| {
                ResourceError::PlatformResourceFailed(
                    "AHardwareBuffer byte size overflowed".to_string(),
                )
            })?;
        let bytes = slice::from_raw_parts_mut(data.cast::<u8>(), byte_len);

        for y in 0..rows {
            for x in 0..stride {
                let offset = (y * stride + x) * SMOKE_BYTES_PER_PIXEL;
                let rgba = if x < cols {
                    pattern_rgba(x as i32, y as i32, width, height, generation)
                } else {
                    [0, 0, 0, 255]
                };
                bytes[offset..offset + SMOKE_BYTES_PER_PIXEL].copy_from_slice(&rgba);
            }
        }

        let mut release_fence = -1;
        let unlock_status = ndk_sys::AHardwareBuffer_unlock(ahb, &mut release_fence);
        if release_fence >= 0 {
            close(release_fence);
        }
        if unlock_status != 0 {
            return Err(ResourceError::PlatformResourceFailed(format!(
                "AHardwareBuffer_unlock failed with status {}",
                unlock_status
            )));
        }

        Ok(())
    }

    fn pattern_rgba(x: i32, y: i32, width: i32, height: i32, generation: u32) -> [u8; 4] {
        let block = 32;
        let mut red = ((x * 255) / width.max(1)) as u8;
        let mut green = ((y * 255) / height.max(1)) as u8;
        let mut blue = ((generation.wrapping_mul(47) % 255) as u8).max(40);

        if ((x / block) + (y / block)) % 2 == 0 {
            red = red.saturating_add(40);
            green = green.saturating_add(20);
        }

        if x > width / 3 && x < (width * 2) / 3 {
            blue = blue.saturating_add(50);
        }
        if y > height / 3 && y < (height * 2) / 3 {
            red = red.saturating_add(30);
        }

        [red, green, blue, 255]
    }
}

#[cfg(target_os = "android")]
fn init_logging() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_min_level(log::Level::Debug)
            .with_tag("flutter"),
    );
}

#[cfg(target_os = "ios")]
fn init_logging() {
    oslog::OsLogger::new("texture_example")
        .level_filter(::log::LevelFilter::Debug)
        .init()
        .ok();
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn init_logging() {
    simple_logger::init_with_level(log::Level::Debug).unwrap();
}

struct Animator {
    texture: Texture<BoxedPixelData>,
    counter: Cell<u32>,
}

struct PixelBufferSource {}

impl PixelBufferSource {
    fn new() -> Self {
        Self {}
    }
}

impl PayloadProvider<BoxedPixelData> for PixelBufferSource {
    fn get_payload(&self) -> BoxedPixelData {
        let rng = fastrand::Rng::new();
        let width = 100i32;
        let height = 100i32;
        let bytes: Vec<u8> = repeat_with(|| rng.u8(..))
            .take((width * height * 4) as usize)
            .collect();
        SimplePixelData::new_boxed(width, height, bytes)
    }
}

impl Animator {
    fn animate(self: &Rc<Self>) {
        self.texture.mark_frame_available().ok();

        let count = self.counter.get();
        self.counter.set(count + 1);

        if count < 120 {
            let self_clone = self.clone();
            RunLoop::current()
                .schedule(Duration::from_millis(100), move || {
                    self_clone.animate();
                })
                .detach();
        }
    }
}

fn init_on_main_thread(engine_handle: i64) -> irondash_texture::Result<i64> {
    let provider = Arc::new(PixelBufferSource::new());
    let texture = Texture::new_with_provider(engine_handle, provider)?;
    let id = texture.id();

    let animator = Rc::new(Animator {
        texture,
        counter: Cell::new(0),
    });
    animator.animate();

    Ok(id)
}

#[no_mangle]
pub extern "C" fn init_texture_example(engine_id: i64, ffi_ptr: *mut c_void, port: i64) {
    init_logging();
    irondash_dart_ffi::irondash_init_ffi(ffi_ptr);
    // Schedule initialization on main thread. When completed return the
    // texture id back to dart through a port.
    RunLoop::sender_for_main_thread().unwrap().send(move || {
        let port = irondash_dart_ffi::DartPort::new(port);
        #[cfg(target_os = "android")]
        {
            match android_smoke::acquire_texture(engine_id) {
                Ok(id) => {
                    port.send(id);
                }
                Err(err) => {
                    error!("Android smoke init failed: {}", err);
                    port.send(DartValue::Null);
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            match init_on_main_thread(engine_id) {
                Ok(id) => {
                    port.send(id);
                }
                Err(err) => {
                    error!("Error {:?}", err);
                    port.send(DartValue::Null);
                }
            }
        }
    });
}

#[no_mangle]
pub extern "C" fn release_texture_example(engine_id: i64, ffi_ptr: *mut c_void, port: i64) {
    init_logging();
    irondash_dart_ffi::irondash_init_ffi(ffi_ptr);

    RunLoop::sender_for_main_thread().unwrap().send(move || {
        let port = irondash_dart_ffi::DartPort::new(port);

        #[cfg(target_os = "android")]
        {
            match android_smoke::release_texture(engine_id) {
                Ok(released) => {
                    port.send(if released { 1i64 } else { 0i64 });
                }
                Err(err) => {
                    error!("Android smoke release failed: {}", err);
                    port.send(0i64);
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = engine_id;
            port.send(1i64);
        }
    });
}
