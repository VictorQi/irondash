use std::{cell::Cell, ffi::c_void, iter::repeat_with, rc::Rc, sync::Arc, time::Duration};

use irondash_dart_ffi::DartValue;
use irondash_run_loop::RunLoop;
use irondash_texture::{BoxedPixelData, PayloadProvider, SimplePixelData, Texture};
use log::error;

#[cfg(target_os = "android")]
mod android_smoke {
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::slice;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    use error_model::ResourceError;
    use ffi_api::{
        irondash_ffi_acquire_shared_texture,
        irondash_ffi_cancel_request,
        irondash_ffi_get_request_state,
        irondash_ffi_init,
        irondash_ffi_process_pending_requests,
        irondash_ffi_pause_request,
        irondash_ffi_register_engine,
        irondash_ffi_release_texture,
        irondash_ffi_resume_request,
        irondash_ffi_try_pop_event,
        irondash_ffi_unregister_engine,
        replace_global_api,
        AndroidFrameBridgeDriver,
        FfiAcquireDecision,
        FfiApi,
        AndroidTextureRegistrationStrategy,
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
    use protocol::{PlatformHandle, SourceId, SourceLifecycleState};
    use request_orchestrator::{BackpressureAction, OrchestratorConfig, RequestOrchestrator};
    use resource_manager::{ResourceManager, ResourceManagerConfig, SourceReleaseStatus};
    use shared_source::SharedSource;
    use thread_dispatcher::ThreadTarget;

    const SMOKE_SOURCE_ID: u64 = 1001;
    const SMOKE_REJECT_SOURCE_ID: u64 = 1002;
    const SMOKE_CONTROL_SOURCE_ID: u64 = 1101;
    const SMOKE_RELEASE_BEFORE_READY_SOURCE_ID: u64 = 1102;
    const SMOKE_DUPLICATE_SOURCE_ID: u64 = 1103;
    const SMOKE_CANCEL_AFTER_READY_SOURCE_ID: u64 = 1104;
    const SMOKE_INTERLEAVED_CONTROL_SOURCE_ID: u64 = 1105;
    const SMOKE_ENGINE_GONE_PENDING_SOURCE_ID: u64 = 1106;
    const SMOKE_CANCEL_MIDFLIGHT_SOURCE_ID: u64 = 1107;
    const SMOKE_LIFECYCLE_SOURCE_IDS: [u64; 3] = [1201, 1202, 1203];
    const SMOKE_WIDTH: i32 = 256;
    const SMOKE_HEIGHT: i32 = 256;
    const SMOKE_BYTES_PER_PIXEL: usize = 4;
    const SMOKE_SOURCE_BYTES: usize =
        (SMOKE_WIDTH as usize) * (SMOKE_HEIGHT as usize) * SMOKE_BYTES_PER_PIXEL;
    const MAX_REQUEST_EVENT_HISTORY: usize = 6;

    static SMOKE_RUNTIME: OnceLock<Arc<FfiApi>> = OnceLock::new();
    static SMOKE_SESSIONS: OnceLock<Mutex<HashMap<i64, SmokeSession>>> = OnceLock::new();
    static SMOKE_DIAGNOSTICS: OnceLock<Mutex<SmokeDiagnosticsState>> = OnceLock::new();

    #[derive(Clone, Copy)]
    struct SmokeSession {
        engine_handle: i64,
        source_id: u64,
        request_id: u64,
        texture_id: i64,
        released: bool,
    }

    #[derive(Clone, Copy)]
    struct TrackedRequest {
        request_id: u64,
        source_id: u64,
    }

    #[derive(Clone)]
    struct SmokeEventSummary {
        event_name: &'static str,
        event_type: u32,
        request_id: u64,
        source_id: u64,
        engine_handle: i64,
        texture_id: i64,
        status: u32,
        error_code: u32,
    }

    #[derive(Clone, Copy, Default)]
    struct SmokeSourceSummary {
        resolve_count: u32,
        latest_generation: u32,
    }

    #[derive(Default)]
    struct SmokeDiagnosticsState {
        last_request_by_engine: HashMap<i64, TrackedRequest>,
        last_event_by_engine: HashMap<i64, SmokeEventSummary>,
        recent_events_by_request: HashMap<u64, Vec<SmokeEventSummary>>,
        source_summary_by_id: HashMap<u64, SmokeSourceSummary>,
    }

    struct AndroidSmokeSourceProvider {
        generation: AtomicU32,
        source_ids: &'static [u64],
    }

    struct SmokeFailingBridgeDriver;

    impl AndroidSmokeSourceProvider {
        fn new(source_ids: &'static [u64]) -> Self {
            Self {
                generation: AtomicU32::new(0),
                source_ids,
            }
        }

        fn next_generation(&self) -> u32 {
            self.generation.fetch_add(1, Ordering::AcqRel) + 1
        }
    }

    impl SourceProvider for AndroidSmokeSourceProvider {
        fn resolve_source(&self, source_id: SourceId) -> Result<Arc<SharedSource>, ResourceError> {
            if !self.source_ids.contains(&source_id.as_u64()) {
                return Err(ResourceError::SourceNotFound(source_id.to_string()));
            }

            let generation = self.next_generation();
            let source = create_smoke_source(source_id, generation)?;
            track_source_resolve(source_id.as_u64(), generation);
            Ok(source)
        }
    }

    impl AndroidFrameBridgeDriver for SmokeFailingBridgeDriver {
        fn copy_to_native_window(
            &self,
            _handle: PlatformHandle,
            _native_window: *mut c_void,
        ) -> Result<(), ResourceError> {
            Err(ResourceError::PlatformResourceFailed(
                "smoke bridge failure".into(),
            ))
        }
    }

    pub fn acquire_texture(engine_handle: i64) -> Result<i64, String> {
        let _runtime = ensure_runtime();
        info!("Smoke acquire start: engine_handle={}", engine_handle);
        release_existing_session(engine_handle);

        let register = irondash_ffi_register_engine(engine_handle);
        if !register.success {
            return Err(format!(
                "register_engine failed for handle {} with error_code={}",
                engine_handle, register.error_code
            ));
        }
        debug!(
            "Smoke register_engine completed: engine_handle={}, success={}, is_registered={}, error_code={}",
            engine_handle,
            register.success,
            register.is_registered,
            register.error_code
        );

        let response = irondash_ffi_acquire_shared_texture(
            SMOKE_SOURCE_ID,
            engine_handle,
            FfiPriorityCode::Visible as u32,
        );
        debug!(
            "Smoke acquire response: decision={}, request_id={}, status={}, texture_id={}, error_code={}",
            response.decision,
            response.request_id,
            response.status,
            response.texture_id,
            response.error_code
        );
        if response.decision == FfiAcquireDecision::Rejected as u32 {
            return Err(format!(
                "acquire_shared_texture rejected with error_code={}",
                response.error_code
            ));
        }

        track_request(engine_handle, response.request_id, SMOKE_SOURCE_ID);

        if response.texture_id >= 0 {
            store_session(
                engine_handle,
                SMOKE_SOURCE_ID,
                response.request_id,
                response.texture_id,
            );
            drain_events(response.request_id);
            info!(
                "Smoke texture ready immediately: request_id={}, texture_id={}",
                response.request_id, response.texture_id
            );
            return Ok(response.texture_id);
        }

        let processed = irondash_ffi_process_pending_requests(1);
        log_request_state("after acquire submit", response.request_id);
        debug!(
            "Smoke request pumped on platform thread: request_id={}, processed={}",
            response.request_id, processed
        );
        log_request_state("after process_pending_requests", response.request_id);

        let texture_id = find_texture_id(response.request_id).ok_or_else(|| {
            format!(
                "smoke request {} did not reach TextureReady after one pump",
                response.request_id
            )
        })?;

        store_session(engine_handle, SMOKE_SOURCE_ID, response.request_id, texture_id);
        info!(
            "Smoke texture ready: request_id={}, texture_id={}",
            response.request_id, texture_id
        );
        Ok(texture_id)
    }

    pub fn release_texture(engine_handle: i64) -> Result<bool, String> {
        let _runtime = ensure_runtime();
        let mut guard = smoke_sessions()
            .lock()
            .expect("smoke session mutex poisoned");
        let Some(session) = guard.get(&engine_handle).copied() else {
            return Ok(false);
        };

        if session.released {
            info!(
                "Smoke release skipped: request already released, pending engine teardown remains for reacquire: engine_handle={}, request_id={}, texture_id={}",
                session.engine_handle,
                session.request_id,
                session.texture_id
            );
            return Ok(false);
        }

        info!(
            "Smoke release start: engine_handle={}, request_id={}, texture_id={}",
            session.engine_handle,
            session.request_id,
            session.texture_id
        );

        let release = irondash_ffi_release_texture(session.request_id, session.engine_handle);
        if release.error_code != 0 {
            return Err(format!(
                "release_texture failed for request {} with error_code={}",
                session.request_id, release.error_code
            ));
        }
        log_request_state("after release_texture", session.request_id);
        debug!(
            "Smoke release result: request_id={}, texture_id={}, release_disposition={}, error_code={}",
            session.request_id,
            session.texture_id,
            release.release_disposition,
            release.error_code
        );
        if let Some(stored) = guard.get_mut(&engine_handle) {
            stored.released = true;
        }
        drop(guard);

        drain_events(session.request_id);
        info!(
            "Smoke texture released without engine teardown: engine_handle={}, request_id={}, texture_id={}, release_disposition={}",
            session.engine_handle,
            session.request_id,
            session.texture_id,
            release.release_disposition
        );

        Ok(matches!(
            release.release_disposition,
            value if value == FfiReleaseDisposition::Released as u32
                || value == FfiReleaseDisposition::NotFound as u32
        ))
    }

    pub fn fetch_snapshot(engine_handle: i64) -> String {
        let _runtime = ensure_runtime();

        let session = smoke_sessions()
            .lock()
            .expect("smoke session mutex poisoned")
            .get(&engine_handle)
            .copied();
        let diagnostics = smoke_diagnostics()
            .lock()
            .expect("smoke diagnostics mutex poisoned");
        let tracked_request = diagnostics.last_request_by_engine.get(&engine_handle).copied();
        let last_event = diagnostics.last_event_by_engine.get(&engine_handle).cloned();
        drop(diagnostics);

        let request_id = session
            .map(|session| session.request_id)
            .or_else(|| tracked_request.map(|tracked| tracked.request_id));
        let request_state = request_id.and_then(request_state);
        let source_id = request_state
            .map(|state| state.source_id)
            .or_else(|| session.map(|session| session.source_id))
            .or_else(|| tracked_request.map(|tracked| tracked.source_id));
        let released = session.map(|session| session.released);
        let source_summary = source_id
            .and_then(|source_id| diagnostics_source_summary(source_id));
        let request_events = request_id
            .and_then(recent_request_events);

        let request_line = match request_state {
            Some(state) => format!(
                "request_state={} ({}) | texture_id={} | error_code={}",
                state.status,
                status_name(state.status),
                state.texture_id,
                state.error_code
            ),
            None => "request_state=- | texture_id=- | error_code=-".to_string(),
        };

        let event_line = match last_event {
            Some(event) => format!(
                "last_event={} ({}) | request_id={} | source_id={} | texture_id={} | status={} ({}) | error_code={}",
                event.event_name,
                event.event_type,
                display_u64(event.request_id),
                event.source_id,
                display_i64(event.texture_id),
                event.status,
                status_name(event.status),
                event.error_code
            ),
            None => "last_event=-".to_string(),
        };

        let source_line = match source_summary {
            Some(summary) => format!(
                "source_resolve_count={} | source_generation={}",
                summary.resolve_count, summary.latest_generation
            ),
            None => "source_resolve_count=- | source_generation=-".to_string(),
        };

        let event_history_line = match request_events {
            Some(events) if !events.is_empty() => format!(
                "request_events={}",
                events
                    .iter()
                    .map(format_event_for_history)
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ),
            _ => "request_events=-".to_string(),
        };

        format!(
            "engine_handle={}\nregistration_strategy=HardwareBufferSeam\ndelivery_path=AndroidHardwareBufferSeam\nsource_bytes={}\nsource_id={}\nrequest_id={}\nactive_session={}\nreleased={}\n{}\n{}\n{}\n{}",
            engine_handle,
            (SMOKE_WIDTH as usize) * (SMOKE_HEIGHT as usize) * SMOKE_BYTES_PER_PIXEL,
            display_optional_u64(source_id),
            display_optional_u64(request_id),
            session.is_some(),
            display_bool(released),
            request_line,
            event_line,
            source_line,
            event_history_line,
        )
    }

    pub fn run_reject_diagnostic(engine_handle: i64) -> Result<String, String> {
        let _runtime = ensure_runtime();
        info!(
            "Smoke reject diagnostic start: engine_handle={}",
            engine_handle
        );
        release_existing_session(engine_handle);

        let diagnostic_api = build_smoke_api(
            AndroidTextureRegistrationStrategy::HardwareBufferSeam,
            &[SMOKE_SOURCE_ID, SMOKE_REJECT_SOURCE_ID],
            Some(1),
            None,
        );

        with_swapped_runtime(diagnostic_api, || {
            let register = irondash_ffi_register_engine(engine_handle);
            if !register.success {
                return Err(format!(
                    "reject diagnostic register_engine failed with error_code={}",
                    register.error_code
                ));
            }

            let first = irondash_ffi_acquire_shared_texture(
                SMOKE_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            track_request(engine_handle, first.request_id, SMOKE_SOURCE_ID);
            debug!(
                "Smoke reject diagnostic first acquire: decision={}, request_id={}, status={}, texture_id={}, error_code={}",
                first.decision,
                first.request_id,
                first.status,
                first.texture_id,
                first.error_code
            );
            if first.decision != FfiAcquireDecision::Accepted as u32 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "reject diagnostic expected first acquire to be accepted, got decision={} error_code={}",
                    first.decision, first.error_code
                ));
            }

            let second = irondash_ffi_acquire_shared_texture(
                SMOKE_REJECT_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            debug!(
                "Smoke reject diagnostic second acquire: decision={}, request_id={}, status={}, texture_id={}, error_code={}",
                second.decision,
                second.request_id,
                second.status,
                second.texture_id,
                second.error_code
            );
            if second.decision != FfiAcquireDecision::Rejected as u32 {
                let _ = irondash_ffi_cancel_request(first.request_id);
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "reject diagnostic expected second acquire to be rejected, got decision={} error_code={}",
                    second.decision, second.error_code
                ));
            }

            let cancel = irondash_ffi_cancel_request(first.request_id);
            debug!(
                "Smoke reject diagnostic cancel result: request_id={}, success={}, status={}, error_code={}",
                first.request_id,
                cancel.success,
                cancel.status,
                cancel.error_code
            );
            drain_events(first.request_id);
            log_request_state("after reject diagnostic cancel", first.request_id);

            let unregister = irondash_ffi_unregister_engine(engine_handle);
            debug!(
                "Smoke reject diagnostic unregister result: engine_handle={}, success={}, is_registered={}, error_code={}",
                engine_handle,
                unregister.success,
                unregister.is_registered,
                unregister.error_code
            );

            info!(
                "Smoke reject diagnostic observed expected rejection: engine_handle={}, accepted_request_id={}, rejected_error_code={}",
                engine_handle,
                first.request_id,
                second.error_code
            );
            Ok(format!(
                "已在真机命中 acquire rejected；accepted_request_id={}，error_code={}。请查看 flutter 日志中的 FFI acquire rejected。",
                first.request_id,
                second.error_code
            ))
        })
    }

    pub fn run_bridge_failure_diagnostic(engine_handle: i64) -> Result<String, String> {
        let _runtime = ensure_runtime();
        info!(
            "Smoke bridge-failure diagnostic start: engine_handle={}",
            engine_handle
        );
        release_existing_session(engine_handle);

        let diagnostic_api = build_smoke_api(
            AndroidTextureRegistrationStrategy::CpuCopyBridge,
            &[SMOKE_SOURCE_ID],
            None,
            Some(Arc::new(SmokeFailingBridgeDriver)),
        );

        with_swapped_runtime(diagnostic_api, || {
            let register = irondash_ffi_register_engine(engine_handle);
            if !register.success {
                return Err(format!(
                    "bridge diagnostic register_engine failed with error_code={}",
                    register.error_code
                ));
            }

            let response = irondash_ffi_acquire_shared_texture(
                SMOKE_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            track_request(engine_handle, response.request_id, SMOKE_SOURCE_ID);
            debug!(
                "Smoke bridge-failure diagnostic acquire: decision={}, request_id={}, status={}, texture_id={}, error_code={}",
                response.decision,
                response.request_id,
                response.status,
                response.texture_id,
                response.error_code
            );
            if response.decision == FfiAcquireDecision::Rejected as u32 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "bridge diagnostic acquire was unexpectedly rejected with error_code={}",
                    response.error_code
                ));
            }

            let processed = irondash_ffi_process_pending_requests(1);
            debug!(
                "Smoke bridge-failure diagnostic processed pending requests: request_id={}, processed={}",
                response.request_id,
                processed
            );
            drain_events(response.request_id);
            log_request_state("after bridge failure diagnostic", response.request_id);

            let mut state = FfiRequestStateSnapshot::default();
            let found = irondash_ffi_get_request_state(response.request_id, &mut state as *mut _);
            if !found {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "bridge diagnostic could not read request state for request_id={}",
                    response.request_id
                ));
            }
            if state.status != 5 || state.texture_id < 0 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "bridge diagnostic expected Registered state after failed bridge, got status={} texture_id={} error_code={}",
                    state.status, state.texture_id, state.error_code
                ));
            }

            let unregister = irondash_ffi_unregister_engine(engine_handle);
            debug!(
                "Smoke bridge-failure diagnostic unregister result: engine_handle={}, success={}, is_registered={}, error_code={}",
                engine_handle,
                unregister.success,
                unregister.is_registered,
                unregister.error_code
            );

            info!(
                "Smoke bridge-failure diagnostic observed expected Registered fallback after bridge error: request_id={}, texture_id={}, status={}, error_code={}",
                response.request_id,
                state.texture_id,
                state.status,
                state.error_code
            );
            Ok(format!(
                "已在真机命中 frame bridge failed；request_id={}，texture_id={}，status={}。请查看 flutter 日志中的 FFI Android frame bridge failed。",
                response.request_id,
                state.texture_id,
                state.status
            ))
        })
    }

    pub fn run_control_plane_diagnostic(engine_handle: i64) -> Result<String, String> {
        let _runtime = ensure_runtime();
        info!(
            "Smoke control-plane diagnostic start: engine_handle={}",
            engine_handle
        );
        release_existing_session(engine_handle);

        let diagnostic_api = build_smoke_api(
            AndroidTextureRegistrationStrategy::HardwareBufferSeam,
            &[
                SMOKE_CONTROL_SOURCE_ID,
                SMOKE_RELEASE_BEFORE_READY_SOURCE_ID,
                SMOKE_DUPLICATE_SOURCE_ID,
            ],
            None,
            None,
        );

        with_swapped_runtime(diagnostic_api, || {
            let register = irondash_ffi_register_engine(engine_handle);
            if !register.success {
                return Err(format!(
                    "control-plane diagnostic register_engine failed with error_code={}",
                    register.error_code
                ));
            }

            let pause_resume_request = irondash_ffi_acquire_shared_texture(
                SMOKE_CONTROL_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            if pause_resume_request.decision != FfiAcquireDecision::Accepted as u32 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "control-plane diagnostic expected initial acquire to be accepted, got decision={} error_code={}",
                    pause_resume_request.decision, pause_resume_request.error_code
                ));
            }
            let _ = drain_event_records();

            let pause = irondash_ffi_pause_request(pause_resume_request.request_id);
            if !pause.success || pause.status != 11 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "control-plane diagnostic expected pause_request to enter Canceling, got success={} status={} error_code={}",
                    pause.success, pause.status, pause.error_code
                ));
            }
            let pause_events = drain_event_records();
            if !has_event_type(&pause_events, FfiEventType::RequestPaused as u32) {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "control-plane diagnostic expected RequestPaused event, got {}",
                    format_event_records(&pause_events)
                ));
            }

            let resume = irondash_ffi_resume_request(pause_resume_request.request_id);
            if !resume.success || resume.status != 2 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "control-plane diagnostic expected resume to re-enter Loading, got success={} status={} error_code={}",
                    resume.success, resume.status, resume.error_code
                ));
            }
            let resume_events = drain_event_records();
            if !has_event_type(&resume_events, FfiEventType::RequestResumed as u32) {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "control-plane diagnostic expected RequestResumed event, got {}",
                    format_event_records(&resume_events)
                ));
            }

            let processed_ready = irondash_ffi_process_pending_requests(1);
            let ready_events = drain_event_records();
            let pause_resume_state = request_state(pause_resume_request.request_id)
                .ok_or_else(|| {
                    format!(
                        "control-plane diagnostic could not read resumed request state for request_id={}",
                        pause_resume_request.request_id
                    )
                })?;
            if processed_ready != 1
                || pause_resume_state.status != 6
                || pause_resume_state.texture_id < 0
                || !has_event_type(&ready_events, FfiEventType::TextureReady as u32)
            {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "control-plane diagnostic pause/resume path did not reach Ready: processed={}, state_status={}, texture_id={}, events={}",
                    processed_ready,
                    pause_resume_state.status,
                    pause_resume_state.texture_id,
                    format_event_records(&ready_events)
                ));
            }

            let _ = irondash_ffi_release_texture(
                pause_resume_request.request_id,
                engine_handle,
            );
            let _ = drain_event_records();

            let release_before_ready = irondash_ffi_acquire_shared_texture(
                SMOKE_RELEASE_BEFORE_READY_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            if release_before_ready.decision != FfiAcquireDecision::Accepted as u32 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "control-plane diagnostic expected release-before-ready acquire to be accepted, got decision={} error_code={}",
                    release_before_ready.decision, release_before_ready.error_code
                ));
            }
            let _ = drain_event_records();

            let release_before_ready_result =
                irondash_ffi_release_texture(release_before_ready.request_id, engine_handle);
            let release_before_ready_events = drain_event_records();
            let late_ready_processed = irondash_ffi_process_pending_requests(1);
            let late_ready_events = drain_event_records();
            let release_before_ready_state = request_state(release_before_ready.request_id)
                .ok_or_else(|| {
                    format!(
                        "control-plane diagnostic could not read release-before-ready state for request_id={}",
                        release_before_ready.request_id
                    )
                })?;
            if release_before_ready_state.status != 9
                || has_event_type(&late_ready_events, FfiEventType::TextureReady as u32)
            {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "control-plane diagnostic late-ready discard failed: processed={}, state_status={}, release_events={}, late_events={}",
                    late_ready_processed,
                    release_before_ready_state.status,
                    format_event_records(&release_before_ready_events),
                    format_event_records(&late_ready_events)
                ));
            }

            let duplicate_first = irondash_ffi_acquire_shared_texture(
                SMOKE_DUPLICATE_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            if duplicate_first.decision != FfiAcquireDecision::Accepted as u32 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "control-plane diagnostic expected duplicate first acquire to be accepted, got decision={} error_code={}",
                    duplicate_first.decision, duplicate_first.error_code
                ));
            }
            let _ = drain_event_records();

            let duplicate_pending = irondash_ffi_acquire_shared_texture(
                SMOKE_DUPLICATE_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            if duplicate_pending.decision != FfiAcquireDecision::Reused as u32
                || duplicate_pending.request_id != duplicate_first.request_id
            {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "control-plane diagnostic expected duplicate pending acquire to reuse request_id={}, got decision={} request_id={}",
                    duplicate_first.request_id,
                    duplicate_pending.decision,
                    duplicate_pending.request_id
                ));
            }

            let duplicate_reuse_events = drain_event_records();
            let duplicate_processed = irondash_ffi_process_pending_requests(1);
            let duplicate_ready_events = drain_event_records();
            let duplicate_ready = irondash_ffi_acquire_shared_texture(
                SMOKE_DUPLICATE_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            let duplicate_state = request_state(duplicate_first.request_id).ok_or_else(|| {
                format!(
                    "control-plane diagnostic could not read duplicate request state for request_id={}",
                    duplicate_first.request_id
                )
            })?;
            if duplicate_processed != 1
                || duplicate_ready.decision != FfiAcquireDecision::Reused as u32
                || duplicate_ready.request_id != duplicate_first.request_id
                || duplicate_ready.status != 6
                || duplicate_state.status != 6
            {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "control-plane diagnostic duplicate acquire path failed: processed={}, ready_decision={}, ready_status={}, state_status={}, events={}",
                    duplicate_processed,
                    duplicate_ready.decision,
                    duplicate_ready.status,
                    duplicate_state.status,
                    format_event_records(&duplicate_ready_events)
                ));
            }

            let _ = irondash_ffi_release_texture(duplicate_first.request_id, engine_handle);
            let _ = drain_event_records();

            let unregister = irondash_ffi_unregister_engine(engine_handle);
            if !unregister.success {
                return Err(format!(
                    "control-plane diagnostic unregister_engine failed with error_code={}",
                    unregister.error_code
                ));
            }
            let _ = drain_event_records();

            Ok(format!(
                "p1_control_plane=PASS\npause_resume_path=pause_request -> resume_request -> Ready\npause_events={}\nresume_events={}\nready_events={}\nrelease_before_ready=PASS | result={} | status={} ({}) | followup_ready_events={}\nduplicate_acquire=PASS | pending_reuse_events={} | ready_events={}\nbackpressure_ui=Use existing Trigger Reject Diagnostic button for the max_pending_requests=1 rejection path",
                format_event_records(&pause_events),
                format_event_records(&resume_events),
                format_event_records(&ready_events),
                release_disposition_name(release_before_ready_result.release_disposition),
                release_before_ready_state.status,
                status_name(release_before_ready_state.status),
                format_event_records(&late_ready_events),
                format_event_records(&duplicate_reuse_events),
                format_event_records(&duplicate_ready_events),
            ))
        })
    }

    pub fn run_full_lifecycle_diagnostic() -> Result<String, String> {
        let budget_bytes = SMOKE_SOURCE_BYTES * 2;
        let manager = ResourceManager::new(ResourceManagerConfig {
            max_bytes: budget_bytes,
            max_sources: 2,
            high_water_mark: 1.0,
        });
        let mut generations = HashMap::<u64, u32>::new();
        let mut plans = HashMap::<u64, ThreadTarget>::new();
        let mut cache_bytes_after_insert = Vec::new();

        for source_id_raw in SMOKE_LIFECYCLE_SOURCE_IDS {
            let source_id = SourceId::new(source_id_raw);
            let generation = next_lifecycle_generation(&mut generations, source_id_raw);
            let source = create_smoke_source(source_id, generation).map_err(|error| {
                format!(
                    "full lifecycle diagnostic could not create source {} generation {}: {:?}",
                    source_id_raw, generation, error
                )
            })?;
            let release_plan = source.release_plan();
            let Some(target_thread) = release_plan.target_thread else {
                return Err(format!(
                    "full lifecycle diagnostic source {} is missing a platform cleaner",
                    source_id_raw
                ));
            };
            plans.insert(source_id_raw, target_thread);
            manager
                .register_source(source_id, source)
                .map_err(|error| {
                    format!(
                        "full lifecycle diagnostic could not register source {}: {:?}",
                        source_id_raw, error
                    )
                })?;
            cache_bytes_after_insert.push(format!("{}:{}", source_id_raw, manager.cache_bytes()));
        }

        let evicted_source_id = SMOKE_LIFECYCLE_SOURCE_IDS[0];
        let evicted_state = manager.query_source_lifecycle_state(SourceId::new(evicted_source_id));
        let evicted_acquire_available = manager.acquire(SourceId::new(evicted_source_id)).is_some();
        if evicted_acquire_available {
            manager.release(SourceId::new(evicted_source_id));
        }

        let mut surviving_sources = Vec::new();
        for source_id_raw in SMOKE_LIFECYCLE_SOURCE_IDS.iter().skip(1).copied() {
            let source_id = SourceId::new(source_id_raw);
            let available = manager.acquire(source_id).is_some();
            if available {
                manager.release(source_id);
            }
            surviving_sources.push(format!("{}:{}", source_id_raw, available));
        }

        let reloaded_generation = next_lifecycle_generation(&mut generations, evicted_source_id);
        let reloaded_source = create_smoke_source(SourceId::new(evicted_source_id), reloaded_generation)
            .map_err(|error| {
                format!(
                    "full lifecycle diagnostic could not recreate evicted source {} generation {}: {:?}",
                    evicted_source_id, reloaded_generation, error
                )
            })?;
        if let Some(target_thread) = reloaded_source.release_plan().target_thread {
            plans.insert(evicted_source_id, target_thread);
        }
        manager
            .register_source(SourceId::new(evicted_source_id), reloaded_source)
            .map_err(|error| {
                format!(
                    "full lifecycle diagnostic could not re-register evicted source {}: {:?}",
                    evicted_source_id, error
                )
            })?;
        let reloaded_acquire_available = manager.acquire(SourceId::new(evicted_source_id)).is_some();
        if reloaded_acquire_available {
            manager.release(SourceId::new(evicted_source_id));
        }

        let cache_after_reload = manager.cache_bytes();
        let deferred_drop_target = thread_target_name(
            plans.get(&evicted_source_id)
                .copied()
                .unwrap_or(ThreadTarget::Platform),
        );
        let final_release = SMOKE_LIFECYCLE_SOURCE_IDS
            .iter()
            .copied()
            .map(|source_id_raw| {
                format!(
                    "{}:{}",
                    source_id_raw,
                    format_source_release_status(manager.release_source(SourceId::new(source_id_raw)))
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");

        Ok(format!(
            "p1_full_lifecycle=PASS\nbyte_budget={}\ncache_bytes_after_insert={}\nevicted_source_id={}\nevicted_state={}\nevicted_acquire_available={}\nsurviving_sources={}\nreloaded_source_id={}\nreloaded_generation={}\nreloaded_acquire_available={}\ncache_bytes_after_reload={}\ndeferred_drop_target={}\nfinal_release={}\nnotes=This host diagnostic proves LRU eviction and reload at the resource-manager/shared-source boundary; pair it with the live preview path for visible render checks.",
            budget_bytes,
            cache_bytes_after_insert.join(" | "),
            evicted_source_id,
            format_source_lifecycle_state(evicted_state),
            evicted_acquire_available,
            surviving_sources.join(" | "),
            evicted_source_id,
            reloaded_generation,
            reloaded_acquire_available,
            cache_after_reload,
            deferred_drop_target,
            final_release,
        ))
    }

    pub fn run_cancel_midflight_diagnostic(engine_handle: i64) -> Result<String, String> {
        let _runtime = ensure_runtime();
        release_existing_session(engine_handle);

        let diagnostic_api = build_smoke_api(
            AndroidTextureRegistrationStrategy::HardwareBufferSeam,
            &[SMOKE_CANCEL_MIDFLIGHT_SOURCE_ID],
            None,
            None,
        );

        with_swapped_runtime(diagnostic_api, || {
            let register = irondash_ffi_register_engine(engine_handle);
            if !register.success {
                return Err(format!(
                    "cancel-midflight diagnostic register_engine failed with error_code={}",
                    register.error_code
                ));
            }

            let acquire = irondash_ffi_acquire_shared_texture(
                SMOKE_CANCEL_MIDFLIGHT_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            if acquire.decision != FfiAcquireDecision::Accepted as u32 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "cancel-midflight diagnostic expected acquire accepted, got decision={} error_code={}",
                    acquire.decision, acquire.error_code
                ));
            }

            let initial_events = drain_event_records();
            let cancel = irondash_ffi_cancel_request(acquire.request_id);
            let cancel_events = drain_event_records();
            let processed = irondash_ffi_process_pending_requests(1);
            let followup_events = drain_event_records();
            let final_state = request_state(acquire.request_id).ok_or_else(|| {
                format!(
                    "cancel-midflight diagnostic could not read final state for request_id={}",
                    acquire.request_id
                )
            })?;

            if !cancel.success
                || cancel.status != 11
                || processed != 0
                || final_state.status != 11
                || final_state.texture_id != -1
                || !has_event_type(&cancel_events, FfiEventType::RequestCanceled as u32)
                || has_event_type(&followup_events, FfiEventType::TextureReady as u32)
            {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "cancel-midflight diagnostic failed: command_status={}, processed={}, final_status={}, texture_id={}, initial_events={}, cancel_events={}, followup_events={}",
                    cancel.status,
                    processed,
                    final_state.status,
                    final_state.texture_id,
                    format_event_records(&initial_events),
                    format_event_records(&cancel_events),
                    format_event_records(&followup_events),
                ));
            }

            let _ = irondash_ffi_unregister_engine(engine_handle);
            let _ = drain_event_records();

            Ok(format!(
                "cancel_midflight=PASS\ninitial_events={}\ncommand_status={} ({})\nfinal_status={} ({})\ntexture_id={}\ncancel_events={}\nfollowup_events={}",
                format_event_records(&initial_events),
                cancel.status,
                status_name(cancel.status),
                final_state.status,
                status_name(final_state.status),
                final_state.texture_id,
                format_event_records(&cancel_events),
                format_event_records(&followup_events),
            ))
        })
    }

    pub fn run_cancel_after_ready_diagnostic(engine_handle: i64) -> Result<String, String> {
        let _runtime = ensure_runtime();
        release_existing_session(engine_handle);

        let diagnostic_api = build_smoke_api(
            AndroidTextureRegistrationStrategy::HardwareBufferSeam,
            &[SMOKE_CANCEL_AFTER_READY_SOURCE_ID],
            None,
            None,
        );

        with_swapped_runtime(diagnostic_api, || {
            let register = irondash_ffi_register_engine(engine_handle);
            if !register.success {
                return Err(format!(
                    "cancel-after-ready diagnostic register_engine failed with error_code={}",
                    register.error_code
                ));
            }

            let acquire = irondash_ffi_acquire_shared_texture(
                SMOKE_CANCEL_AFTER_READY_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            if acquire.decision != FfiAcquireDecision::Accepted as u32 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "cancel-after-ready diagnostic expected acquire accepted, got decision={} error_code={}",
                    acquire.decision, acquire.error_code
                ));
            }

            let _ = drain_event_records();
            let processed = irondash_ffi_process_pending_requests(1);
            let ready_events = drain_event_records();
            let ready_state = request_state(acquire.request_id).ok_or_else(|| {
                format!(
                    "cancel-after-ready diagnostic could not read ready state for request_id={}",
                    acquire.request_id
                )
            })?;
            if processed != 1 || ready_state.status != 6 || ready_state.texture_id < 0 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "cancel-after-ready diagnostic expected Ready before cancel, got processed={} status={} texture_id={} events={}",
                    processed,
                    ready_state.status,
                    ready_state.texture_id,
                    format_event_records(&ready_events)
                ));
            }

            let cancel = irondash_ffi_cancel_request(acquire.request_id);
            let cancel_events = drain_event_records();
            let state_after_cancel = request_state(acquire.request_id).ok_or_else(|| {
                format!(
                    "cancel-after-ready diagnostic could not read post-cancel state for request_id={}",
                    acquire.request_id
                )
            })?;
            if !cancel.success
                || cancel.status != 6
                || !cancel_events.is_empty()
                || state_after_cancel.status != 6
                || state_after_cancel.texture_id != ready_state.texture_id
            {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "cancel-after-ready diagnostic expected silent no-op, got command_status={}, state_status={}, cancel_events={}, texture_before={}, texture_after={}",
                    cancel.status,
                    state_after_cancel.status,
                    format_event_records(&cancel_events),
                    ready_state.texture_id,
                    state_after_cancel.texture_id,
                ));
            }

            let _ = irondash_ffi_release_texture(acquire.request_id, engine_handle);
            let _ = drain_event_records();
            let _ = irondash_ffi_unregister_engine(engine_handle);
            let _ = drain_event_records();

            Ok(format!(
                "cancel_after_ready=PASS\ncommand_status={} ({})\nstate_status={} ({})\ntexture_id={}\ncancel_events={}",
                cancel.status,
                status_name(cancel.status),
                state_after_cancel.status,
                status_name(state_after_cancel.status),
                state_after_cancel.texture_id,
                format_event_records(&cancel_events),
            ))
        })
    }

    pub fn run_interleaved_pause_resume_diagnostic(engine_handle: i64) -> Result<String, String> {
        let _runtime = ensure_runtime();
        release_existing_session(engine_handle);

        let diagnostic_api = build_smoke_api(
            AndroidTextureRegistrationStrategy::HardwareBufferSeam,
            &[SMOKE_INTERLEAVED_CONTROL_SOURCE_ID],
            None,
            None,
        );

        with_swapped_runtime(diagnostic_api, || {
            let register = irondash_ffi_register_engine(engine_handle);
            if !register.success {
                return Err(format!(
                    "interleaved pause/resume diagnostic register_engine failed with error_code={}",
                    register.error_code
                ));
            }

            let acquire = irondash_ffi_acquire_shared_texture(
                SMOKE_INTERLEAVED_CONTROL_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            if acquire.decision != FfiAcquireDecision::Accepted as u32 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "interleaved pause/resume diagnostic expected acquire accepted, got decision={} error_code={}",
                    acquire.decision, acquire.error_code
                ));
            }

            let _ = drain_event_records();
            let pause = irondash_ffi_pause_request(acquire.request_id);
            let resume = irondash_ffi_resume_request(acquire.request_id);
            let control_events = drain_event_records();
            let processed = irondash_ffi_process_pending_requests(1);
            let ready_events = drain_event_records();
            let final_state = request_state(acquire.request_id).ok_or_else(|| {
                format!(
                    "interleaved pause/resume diagnostic could not read final state for request_id={}",
                    acquire.request_id
                )
            })?;

            if !pause.success
                || !resume.success
                || processed != 1
                || final_state.status != 6
                || final_state.texture_id < 0
                || !has_event_type(&control_events, FfiEventType::RequestPaused as u32)
                || !has_event_type(&control_events, FfiEventType::RequestResumed as u32)
                || !has_event_type(&ready_events, FfiEventType::TextureReady as u32)
            {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "interleaved pause/resume diagnostic failed: pause_status={}, resume_status={}, processed={}, final_status={}, control_events={}, ready_events={}",
                    pause.status,
                    resume.status,
                    processed,
                    final_state.status,
                    format_event_records(&control_events),
                    format_event_records(&ready_events),
                ));
            }

            let _ = irondash_ffi_release_texture(acquire.request_id, engine_handle);
            let _ = drain_event_records();
            let _ = irondash_ffi_unregister_engine(engine_handle);
            let _ = drain_event_records();

            Ok(format!(
                "interleaved_pause_resume=PASS\npause_status={} ({})\nresume_status={} ({})\ncontrol_events={}\nready_events={}\nfinal_status={} ({}) | texture_id={}",
                pause.status,
                status_name(pause.status),
                resume.status,
                status_name(resume.status),
                format_event_records(&control_events),
                format_event_records(&ready_events),
                final_state.status,
                status_name(final_state.status),
                final_state.texture_id,
            ))
        })
    }

    pub fn run_engine_gone_pending_diagnostic(engine_handle: i64) -> Result<String, String> {
        let _runtime = ensure_runtime();
        release_existing_session(engine_handle);

        let diagnostic_api = build_smoke_api(
            AndroidTextureRegistrationStrategy::HardwareBufferSeam,
            &[SMOKE_ENGINE_GONE_PENDING_SOURCE_ID],
            None,
            None,
        );

        with_swapped_runtime(diagnostic_api, || {
            let register = irondash_ffi_register_engine(engine_handle);
            if !register.success {
                return Err(format!(
                    "engine-gone-pending diagnostic register_engine failed with error_code={}",
                    register.error_code
                ));
            }

            let acquire = irondash_ffi_acquire_shared_texture(
                SMOKE_ENGINE_GONE_PENDING_SOURCE_ID,
                engine_handle,
                FfiPriorityCode::Visible as u32,
            );
            if acquire.decision != FfiAcquireDecision::Accepted as u32 {
                let _ = irondash_ffi_unregister_engine(engine_handle);
                return Err(format!(
                    "engine-gone-pending diagnostic expected acquire accepted, got decision={} error_code={}",
                    acquire.decision, acquire.error_code
                ));
            }

            let initial_events = drain_event_records();
            let unregister = irondash_ffi_unregister_engine(engine_handle);
            let engine_gone_events = drain_event_records();
            let processed = irondash_ffi_process_pending_requests(1);
            let followup_events = drain_event_records();
            let state_after_unregister = request_state(acquire.request_id);

            if !unregister.success
                || processed != 0
                || state_after_unregister.is_some()
                || !has_event_type(&engine_gone_events, FfiEventType::EngineGone as u32)
                || has_event_type(&followup_events, FfiEventType::TextureReady as u32)
            {
                return Err(format!(
                    "engine-gone-pending diagnostic failed: processed={}, state_present={}, initial_events={}, engine_gone_events={}, followup_events={}",
                    processed,
                    state_after_unregister.is_some(),
                    format_event_records(&initial_events),
                    format_event_records(&engine_gone_events),
                    format_event_records(&followup_events),
                ));
            }

            Ok(format!(
                "engine_gone_pending=PASS\ninitial_events={}\nengine_gone_events={}\nfollowup_events={}\nrequest_state_after_unregister={}",
                format_event_records(&initial_events),
                format_event_records(&engine_gone_events),
                format_event_records(&followup_events),
                if state_after_unregister.is_some() { "present" } else { "absent" },
            ))
        })
    }

    fn next_lifecycle_generation(
        generations: &mut HashMap<u64, u32>,
        source_id: u64,
    ) -> u32 {
        let generation = generations.entry(source_id).or_insert(0);
        *generation += 1;
        *generation
    }

    fn ensure_runtime() -> Arc<FfiApi> {
        SMOKE_RUNTIME
            .get_or_init(|| {
                irondash_ffi_init();
                debug!(
                    "Smoke host fixes the Android strategy to HardwareBufferSeam so M0 replays the documented scheme-1 baseline"
                );
                let strategy = AndroidTextureRegistrationStrategy::HardwareBufferSeam;
                let api = build_smoke_api(
                    strategy,
                    &[SMOKE_SOURCE_ID],
                    None,
                    None,
                );
                let _ = replace_global_api(api.clone());
                debug!("Smoke runtime initialized with Android strategy: {:?}", strategy);
                api
            })
            .clone()
    }

    fn build_smoke_api(
        strategy: AndroidTextureRegistrationStrategy,
        source_ids: &'static [u64],
        max_pending_requests: Option<usize>,
        bridge_driver: Option<Arc<dyn AndroidFrameBridgeDriver>>,
    ) -> Arc<FfiApi> {
        let mut builder = FfiApi::builder()
            .with_source_provider(Arc::new(AndroidSmokeSourceProvider::new(source_ids)))
            .with_android_texture_registration_strategy(strategy)
            .with_event_callback(Arc::new(log_ffi_event));

        if let Some(max_pending_requests) = max_pending_requests {
            let config = OrchestratorConfig {
                max_pending_requests,
                backpressure_action: BackpressureAction::Reject,
                ..OrchestratorConfig::default()
            };
            builder = builder.with_orchestrator(RequestOrchestrator::new(config));
        }

        if let Some(bridge_driver) = bridge_driver {
            builder = builder.with_android_frame_bridge(bridge_driver);
        }

        Arc::new(builder.build())
    }

    fn with_swapped_runtime<T>(
        diagnostic_api: Arc<FfiApi>,
        run: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let previous = replace_global_api(diagnostic_api);
        let result = run();
        let _ = replace_global_api(previous);
        result
    }

    fn smoke_sessions() -> &'static Mutex<HashMap<i64, SmokeSession>> {
        SMOKE_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn smoke_diagnostics() -> &'static Mutex<SmokeDiagnosticsState> {
        SMOKE_DIAGNOSTICS.get_or_init(|| Mutex::new(SmokeDiagnosticsState::default()))
    }

    fn track_source_resolve(source_id: u64, generation: u32) {
        let mut diagnostics = smoke_diagnostics()
            .lock()
            .expect("smoke diagnostics mutex poisoned");
        let entry = diagnostics
            .source_summary_by_id
            .entry(source_id)
            .or_default();
        entry.resolve_count = entry.resolve_count.saturating_add(1);
        entry.latest_generation = generation;
    }

    fn diagnostics_source_summary(source_id: u64) -> Option<SmokeSourceSummary> {
        smoke_diagnostics()
            .lock()
            .expect("smoke diagnostics mutex poisoned")
            .source_summary_by_id
            .get(&source_id)
            .copied()
    }

    fn recent_request_events(request_id: u64) -> Option<Vec<SmokeEventSummary>> {
        smoke_diagnostics()
            .lock()
            .expect("smoke diagnostics mutex poisoned")
            .recent_events_by_request
            .get(&request_id)
            .cloned()
    }

    fn store_session(engine_handle: i64, source_id: u64, request_id: u64, texture_id: i64) {
        track_request(engine_handle, request_id, source_id);
        let mut guard = smoke_sessions().lock().expect("smoke session mutex poisoned");
        guard.insert(
            engine_handle,
            SmokeSession {
                engine_handle,
                source_id,
                request_id,
                texture_id,
                released: false,
            },
        );
    }

    fn track_request(engine_handle: i64, request_id: u64, source_id: u64) {
        smoke_diagnostics()
            .lock()
            .expect("smoke diagnostics mutex poisoned")
            .last_request_by_engine
            .insert(
                engine_handle,
                TrackedRequest {
                    request_id,
                    source_id,
                },
            );
    }

    fn release_existing_session(engine_handle: i64) {
        let session = {
            let mut guard = smoke_sessions().lock().expect("smoke session mutex poisoned");
            guard.remove(&engine_handle)
        };

        let Some(session) = session else {
            return;
        };

        debug!(
            "Smoke best-effort teardown before reacquire: engine_handle={}, request_id={}, texture_id={}, released={}",
            session.engine_handle,
            session.request_id,
            session.texture_id,
            session.released
        );

        if !session.released {
            let release = irondash_ffi_release_texture(session.request_id, session.engine_handle);
            if release.error_code != 0 {
                warn!(
                    "Best-effort release before reacquire failed: request_id={}, error_code={}",
                    session.request_id, release.error_code
                );
            } else {
                debug!(
                    "Best-effort release before reacquire completed: engine_handle={}, request_id={}, texture_id={}, release_disposition={}",
                    session.engine_handle,
                    session.request_id,
                    session.texture_id,
                    release.release_disposition
                );
            }
            log_request_state("after best-effort release", session.request_id);
        } else {
            debug!(
                "Smoke best-effort teardown before reacquire skipping release call because request is already released: engine_handle={}, request_id={}, texture_id={}",
                session.engine_handle,
                session.request_id,
                session.texture_id
            );
        }

        debug!(
            "Smoke best-effort teardown calling unregister_engine: engine_handle={}, request_id={}",
            session.engine_handle,
            session.request_id
        );
        let unregister = irondash_ffi_unregister_engine(session.engine_handle);
        if !unregister.success && unregister.error_code != 0 {
            warn!(
                "Best-effort unregister before reacquire failed: engine_handle={}, error_code={}",
                session.engine_handle, unregister.error_code
            );
        } else {
            debug!(
                "Smoke best-effort unregister before reacquire completed: engine_handle={}, success={}, is_registered={}, error_code={}",
                session.engine_handle,
                unregister.success,
                unregister.is_registered,
                unregister.error_code
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

    fn log_request_state(stage: &str, request_id: u64) {
        let mut state = FfiRequestStateSnapshot::default();
        let found = irondash_ffi_get_request_state(request_id, &mut state as *mut _);
        debug!(
            "Smoke request state {}: request_id={}, found={}, status={}, texture_id={}, error_code={}",
            stage,
            request_id,
            found,
            state.status,
            state.texture_id,
            state.error_code
        );
    }

    fn drain_event_records() -> Vec<FfiEventRecord> {
        let mut events = Vec::new();

        loop {
            let mut event = FfiEventRecord::default();
            if !irondash_ffi_try_pop_event(&mut event as *mut _) {
                break;
            }

            debug!(
                "smoke ffi event: type={}, request_id={}, texture_id={}, status={}, error_code={}",
                event.event_type, event.request_id, event.texture_id, event.status, event.error_code
            );

            events.push(event);
        }

        events
    }

    fn drain_events(request_id: u64) -> Option<i64> {
        let mut ready_texture_id = None;

        for event in drain_event_records() {
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

        if let Some(summary) = summarize_event(event) {
            let mut diagnostics = smoke_diagnostics()
                .lock()
                .expect("smoke diagnostics mutex poisoned");
            diagnostics
                .last_event_by_engine
                .insert(summary.engine_handle, summary.clone());
            if summary.event_name != "EngineGone" {
                let events = diagnostics
                    .recent_events_by_request
                    .entry(summary.request_id)
                    .or_default();
                events.push(summary);
                if events.len() > MAX_REQUEST_EVENT_HISTORY {
                    let overflow = events.len() - MAX_REQUEST_EVENT_HISTORY;
                    events.drain(0..overflow);
                }
            }
        }
    }

    fn format_event_for_history(event: &SmokeEventSummary) -> String {
        match event.event_name {
            "StatusChanged" => format!(
                "{}:{}",
                event.event_name,
                status_name(event.status)
            ),
            "TextureReady" => format!(
                "{}:{}",
                event.event_name,
                display_i64(event.texture_id)
            ),
            _ => event.event_name.to_string(),
        }
    }

    fn summarize_event(event: FfiEvent) -> Option<SmokeEventSummary> {
        let event_name = event.event_type_name();
        let record = event.to_c();
        let request_state = if event_name == "EngineGone" {
            None
        } else {
            request_state(record.request_id)
        };
        let engine_handle = if record.engine_handle != 0 {
            record.engine_handle
        } else {
            request_state.map(|state| state.engine_handle)?
        };

        Some(SmokeEventSummary {
            event_name,
            event_type: record.event_type,
            request_id: record.request_id,
            source_id: if record.source_id != 0 {
                record.source_id
            } else {
                request_state.map(|state| state.source_id).unwrap_or_default()
            },
            engine_handle,
            texture_id: if record.texture_id >= 0 {
                record.texture_id
            } else {
                request_state.map(|state| state.texture_id).unwrap_or(-1)
            },
            status: if record.status != 0 {
                record.status
            } else {
                request_state.map(|state| state.status).unwrap_or_default()
            },
            error_code: if record.error_code != 0 {
                record.error_code
            } else {
                request_state.map(|state| state.error_code).unwrap_or_default()
            },
        })
    }

    fn request_state(request_id: u64) -> Option<FfiRequestStateSnapshot> {
        let mut state = FfiRequestStateSnapshot::default();
        if irondash_ffi_get_request_state(request_id, &mut state as *mut _) {
            Some(state)
        } else {
            None
        }
    }

    fn status_name(status: u32) -> &'static str {
        match status {
            1 => "Pending",
            2 => "Loading",
            3 => "Loaded",
            4 => "Registering",
            5 => "Registered",
            6 => "Ready",
            7 => "Unregistering",
            8 => "Unloading",
            9 => "Unloaded",
            10 => "Failed",
            11 => "Canceling",
            12 => "Canceled",
            _ => "Unknown",
        }
    }

    fn cancel_reason_name(cancel_reason: u32) -> &'static str {
        match cancel_reason {
            1 => "UserRequest",
            2 => "EngineDestroyed",
            3 => "SourceEvicted",
            4 => "Timeout",
            5 => "Backpressure",
            6 => "Other",
            _ => "None",
        }
    }

    fn release_disposition_name(release_disposition: u32) -> &'static str {
        match release_disposition {
            value if value == FfiReleaseDisposition::Released as u32 => "Released",
            value if value == FfiReleaseDisposition::Deferred as u32 => "Deferred",
            value if value == FfiReleaseDisposition::NotFound as u32 => "NotFound",
            _ => "None",
        }
    }

    fn thread_target_name(target: ThreadTarget) -> &'static str {
        match target {
            ThreadTarget::Platform => "Platform",
            ThreadTarget::Raster => "Raster",
            ThreadTarget::Worker => "Worker",
        }
    }

    fn has_event_type(events: &[FfiEventRecord], event_type: u32) -> bool {
        events.iter().any(|event| event.event_type == event_type)
    }

    fn format_event_records(events: &[FfiEventRecord]) -> String {
        if events.is_empty() {
            return "-".to_string();
        }

        events
            .iter()
            .map(format_event_record)
            .collect::<Vec<_>>()
            .join(" -> ")
    }

    fn format_event_record(event: &FfiEventRecord) -> String {
        match event.event_type {
            value if value == FfiEventType::TextureReady as u32 => {
                format!("TextureReady:{}", display_i64(event.texture_id))
            }
            value if value == FfiEventType::StatusChanged as u32 => {
                format!("StatusChanged:{}", status_name(event.status))
            }
            value if value == FfiEventType::RequestResumed as u32 => {
                format!("RequestResumed:{}", status_name(event.status))
            }
            value if value == FfiEventType::RequestPaused as u32 => {
                format!("RequestPaused:{}", status_name(event.status))
            }
            value if value == FfiEventType::RequestCanceled as u32 => format!(
                "RequestCanceled:{}:{}",
                cancel_reason_name(event.cancel_reason),
                status_name(event.status)
            ),
            value if value == FfiEventType::ResourceReleased as u32 => format!(
                "ResourceReleased:{}:{}",
                release_disposition_name(event.release_disposition),
                status_name(event.status)
            ),
            value if value == FfiEventType::EngineGone as u32 => {
                format!("EngineGone:{}", event.engine_handle)
            }
            _ => format!("Event{}", event.event_type),
        }
    }

    fn format_source_lifecycle_state(state: Option<SourceLifecycleState>) -> String {
        match state {
            Some(state) => format!("{:?}", state),
            None => "-".to_string(),
        }
    }

    fn format_source_release_status(status: SourceReleaseStatus) -> String {
        match status {
            SourceReleaseStatus::Released {
                lifecycle_state,
                outcome,
            } => format!("Released:{:?}/{:?}", lifecycle_state, outcome),
            SourceReleaseStatus::Deferred {
                lifecycle_state,
                active_borrows,
            } => format!("Deferred:{:?}/{}", lifecycle_state, active_borrows),
            SourceReleaseStatus::NotFound { last_known_state } => {
                format!("NotFound:{}", format_source_lifecycle_state(last_known_state))
            }
        }
    }

    fn display_u64(value: u64) -> String {
        value.to_string()
    }

    fn display_optional_u64(value: Option<u64>) -> String {
        value.map(display_u64).unwrap_or_else(|| "-".to_string())
    }

    fn display_i64(value: i64) -> String {
        if value < 0 {
            "-".to_string()
        } else {
            value.to_string()
        }
    }

    fn display_bool(value: Option<bool>) -> &'static str {
        match value {
            Some(true) => "true",
            Some(false) => "false",
            None => "-",
        }
    }

    fn create_smoke_source(
        source_id: SourceId,
        generation: u32,
    ) -> Result<Arc<SharedSource>, ResourceError> {
        let cleaner = Arc::new(AndroidPlatformCleaner::new());
        info!(
            "Creating smoke AHardwareBuffer source: source_id={}, generation={}, size={}x{}",
            source_id.as_u64(),
            generation,
            SMOKE_WIDTH,
            SMOKE_HEIGHT
        );
        let ahb_ptr = unsafe { allocate_ahardware_buffer(SMOKE_WIDTH, SMOKE_HEIGHT)? };

        if let Err(err) = unsafe { fill_ahardware_buffer(ahb_ptr, SMOKE_WIDTH, SMOKE_HEIGHT, generation) } {
            unsafe {
                ndk_sys::AHardwareBuffer_release(ahb_ptr.cast::<AHardwareBuffer>());
            }
            return Err(err);
        }

        let handle = unsafe { AndroidPlatformHandle::from_ahb(ahb_ptr, SMOKE_WIDTH, SMOKE_HEIGHT) };
        debug!(
            "Created smoke AHardwareBuffer handle: source_id={}, generation={}, ahb_ptr={:?}, bytes={}",
            source_id.as_u64(),
            generation,
            ahb_ptr,
            SMOKE_SOURCE_BYTES
        );
        SharedSource::new_with_deferred_drop(
            source_id,
            handle.into(),
            SMOKE_SOURCE_BYTES,
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

        debug!(
            "AHardwareBuffer_allocate succeeded: ptr={:?}, size={}x{}, usage=0x{:x}",
            buffer,
            width,
            height,
            desc.usage
        );

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

        debug!(
            "Filling AHardwareBuffer: ptr={:?}, generation={}, desc={}x{}, stride_pixels={}, byte_len={}",
            ahb_ptr,
            generation,
            desc.width,
            desc.height,
            desc.stride,
            byte_len
        );

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

        debug!(
            "Filled AHardwareBuffer successfully: ptr={:?}, generation={}, stride_pixels={}, rows={}",
            ahb_ptr,
            generation,
            stride,
            rows
        );

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

#[no_mangle]
pub extern "C" fn run_reject_diagnostic_example(engine_id: i64, ffi_ptr: *mut c_void, port: i64) {
    init_logging();
    irondash_dart_ffi::irondash_init_ffi(ffi_ptr);

    RunLoop::sender_for_main_thread().unwrap().send(move || {
        let port = irondash_dart_ffi::DartPort::new(port);

        #[cfg(target_os = "android")]
        {
            match android_smoke::run_reject_diagnostic(engine_id) {
                Ok(message) => {
                    let _ = port.send(message);
                }
                Err(err) => {
                    error!("Android smoke reject diagnostic failed: {}", err);
                    let _ = port.send(format!("reject diagnostic failed: {}", err));
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = engine_id;
            port.send("reject diagnostic is Android-only".to_string());
        }
    });
}

#[no_mangle]
pub extern "C" fn run_bridge_failure_diagnostic_example(
    engine_id: i64,
    ffi_ptr: *mut c_void,
    port: i64,
) {
    init_logging();
    irondash_dart_ffi::irondash_init_ffi(ffi_ptr);

    RunLoop::sender_for_main_thread().unwrap().send(move || {
        let port = irondash_dart_ffi::DartPort::new(port);

        #[cfg(target_os = "android")]
        {
            match android_smoke::run_bridge_failure_diagnostic(engine_id) {
                Ok(message) => {
                    let _ = port.send(message);
                }
                Err(err) => {
                    error!("Android smoke bridge diagnostic failed: {}", err);
                    let _ = port.send(format!("bridge diagnostic failed: {}", err));
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = engine_id;
            port.send("bridge diagnostic is Android-only".to_string());
        }
    });
}

#[no_mangle]
pub extern "C" fn run_control_plane_diagnostic_example(
    engine_id: i64,
    ffi_ptr: *mut c_void,
    port: i64,
) {
    init_logging();
    irondash_dart_ffi::irondash_init_ffi(ffi_ptr);

    RunLoop::sender_for_main_thread().unwrap().send(move || {
        let port = irondash_dart_ffi::DartPort::new(port);

        #[cfg(target_os = "android")]
        {
            match android_smoke::run_control_plane_diagnostic(engine_id) {
                Ok(message) => {
                    let _ = port.send(message);
                }
                Err(err) => {
                    error!("Android smoke control-plane diagnostic failed: {}", err);
                    let _ = port.send(format!("control-plane diagnostic failed: {}", err));
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = engine_id;
            let _ = port.send("control-plane diagnostic is Android-only".to_string());
        }
    });
}

#[no_mangle]
pub extern "C" fn run_full_lifecycle_diagnostic_example(
    engine_id: i64,
    ffi_ptr: *mut c_void,
    port: i64,
) {
    init_logging();
    irondash_dart_ffi::irondash_init_ffi(ffi_ptr);

    RunLoop::sender_for_main_thread().unwrap().send(move || {
        let port = irondash_dart_ffi::DartPort::new(port);

        #[cfg(target_os = "android")]
        {
            let _ = engine_id;
            match android_smoke::run_full_lifecycle_diagnostic() {
                Ok(message) => {
                    let _ = port.send(message);
                }
                Err(err) => {
                    error!("Android smoke full lifecycle diagnostic failed: {}", err);
                    let _ = port.send(format!("full lifecycle diagnostic failed: {}", err));
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = engine_id;
            let _ = port.send("full lifecycle diagnostic is Android-only".to_string());
        }
    });
}

#[no_mangle]
pub extern "C" fn run_cancel_midflight_diagnostic_example(
    engine_id: i64,
    ffi_ptr: *mut c_void,
    port: i64,
) {
    init_logging();
    irondash_dart_ffi::irondash_init_ffi(ffi_ptr);

    RunLoop::sender_for_main_thread().unwrap().send(move || {
        let port = irondash_dart_ffi::DartPort::new(port);

        #[cfg(target_os = "android")]
        {
            match android_smoke::run_cancel_midflight_diagnostic(engine_id) {
                Ok(message) => {
                    let _ = port.send(message);
                }
                Err(err) => {
                    error!("Android smoke cancel-midflight diagnostic failed: {}", err);
                    let _ = port.send(format!("cancel-midflight diagnostic failed: {}", err));
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = engine_id;
            let _ = port.send("cancel-midflight diagnostic is Android-only".to_string());
        }
    });
}

#[no_mangle]
pub extern "C" fn run_cancel_after_ready_diagnostic_example(
    engine_id: i64,
    ffi_ptr: *mut c_void,
    port: i64,
) {
    init_logging();
    irondash_dart_ffi::irondash_init_ffi(ffi_ptr);

    RunLoop::sender_for_main_thread().unwrap().send(move || {
        let port = irondash_dart_ffi::DartPort::new(port);

        #[cfg(target_os = "android")]
        {
            match android_smoke::run_cancel_after_ready_diagnostic(engine_id) {
                Ok(message) => {
                    let _ = port.send(message);
                }
                Err(err) => {
                    error!("Android smoke cancel-after-ready diagnostic failed: {}", err);
                    let _ = port.send(format!("cancel-after-ready diagnostic failed: {}", err));
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = engine_id;
            let _ = port.send("cancel-after-ready diagnostic is Android-only".to_string());
        }
    });
}

#[no_mangle]
pub extern "C" fn run_interleaved_pause_resume_diagnostic_example(
    engine_id: i64,
    ffi_ptr: *mut c_void,
    port: i64,
) {
    init_logging();
    irondash_dart_ffi::irondash_init_ffi(ffi_ptr);

    RunLoop::sender_for_main_thread().unwrap().send(move || {
        let port = irondash_dart_ffi::DartPort::new(port);

        #[cfg(target_os = "android")]
        {
            match android_smoke::run_interleaved_pause_resume_diagnostic(engine_id) {
                Ok(message) => {
                    let _ = port.send(message);
                }
                Err(err) => {
                    error!("Android smoke interleaved pause/resume diagnostic failed: {}", err);
                    let _ = port.send(format!("interleaved pause/resume diagnostic failed: {}", err));
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = engine_id;
            let _ = port.send("interleaved pause/resume diagnostic is Android-only".to_string());
        }
    });
}

#[no_mangle]
pub extern "C" fn run_engine_gone_pending_diagnostic_example(
    engine_id: i64,
    ffi_ptr: *mut c_void,
    port: i64,
) {
    init_logging();
    irondash_dart_ffi::irondash_init_ffi(ffi_ptr);

    RunLoop::sender_for_main_thread().unwrap().send(move || {
        let port = irondash_dart_ffi::DartPort::new(port);

        #[cfg(target_os = "android")]
        {
            match android_smoke::run_engine_gone_pending_diagnostic(engine_id) {
                Ok(message) => {
                    let _ = port.send(message);
                }
                Err(err) => {
                    error!("Android smoke engine-gone-pending diagnostic failed: {}", err);
                    let _ = port.send(format!("engine-gone-pending diagnostic failed: {}", err));
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = engine_id;
            let _ = port.send("engine-gone-pending diagnostic is Android-only".to_string());
        }
    });
}

#[no_mangle]
pub extern "C" fn fetch_smoke_snapshot_example(engine_id: i64, ffi_ptr: *mut c_void, port: i64) {
    init_logging();
    irondash_dart_ffi::irondash_init_ffi(ffi_ptr);

    RunLoop::sender_for_main_thread().unwrap().send(move || {
        let port = irondash_dart_ffi::DartPort::new(port);

        #[cfg(target_os = "android")]
        {
            let _ = port.send(android_smoke::fetch_snapshot(engine_id));
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = engine_id;
            let _ = port.send("smoke snapshot is Android-only".to_string());
        }
    });
}
