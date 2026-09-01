//! Generation-scoped system and WeChat OCR provider execution.

use super::*;

pub(super) struct WechatFullFrame {
    pub(super) geometry: CaptureGeometry,
    pub(super) bitmap: Option<windows::Graphics::Imaging::SoftwareBitmap>,
    pub(super) _ledger: crate::support::perf_probe::ResourceGuard,
}

pub(super) enum WechatInput {
    Frame(WechatFullFrame),
    CancelWake,
    Failed(String),
}

impl WechatFullFrame {
    fn take_bitmap(&mut self) -> Result<windows::Graphics::Imaging::SoftwareBitmap, VisionError> {
        self.bitmap
            .take()
            .ok_or_else(|| VisionError::Cleanup("WeChat OCR bitmap was already consumed".into()))
    }
}

impl Drop for WechatFullFrame {
    fn drop(&mut self) {
        if let Some(bitmap) = self.bitmap.take()
            && let Err(error) = bitmap.Close()
        {
            crate::support::logging::report_error(
                "windows-vision",
                format!("cannot close unused WeChat OCR bitmap: {error}"),
            );
        }
    }
}

pub(super) struct SystemOcrTile {
    pub(super) geometry: CaptureGeometry,
    pub(super) core_bounds: Rect,
    pub(super) bitmap: SharedSoftwareBitmap,
}

pub(super) enum SystemOcrInput {
    Begin { tile_count: usize },
    Tile(SystemOcrTile),
    CompletionWake,
    CancelWake,
    Failed(String),
    Done,
}

pub(super) struct SystemOcrSubmission {
    pub(super) sender: mpsc::SyncSender<SystemOcrInput>,
    pub(super) credits: mpsc::Receiver<()>,
    pub(super) layout: SystemOcrLayout,
    pub(super) deadline: Instant,
}

pub(super) struct SystemOcrCompletion(AtomicU8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SystemOcrCompletionStatus {
    Completed,
    Canceled,
    Error,
    NonTerminal,
}

impl SystemOcrCompletion {
    pub(super) fn pending() -> Self {
        Self(AtomicU8::new(0))
    }
    pub(super) fn publish(&self, status: AsyncStatus) {
        let value = match status {
            AsyncStatus::Completed => 1,
            AsyncStatus::Canceled => 2,
            AsyncStatus::Error => 3,
            _ => 4,
        };
        self.0.store(value, Ordering::Release);
    }
    pub(super) fn take(&self) -> Option<SystemOcrCompletionStatus> {
        // The provider is the only consumer and each operation completes once.
        // Load first so scanning pending tiles does not issue a locked RMW for
        // the overwhelmingly common not-ready state.
        let value = self.0.load(Ordering::Acquire);
        if value == 0 {
            return None;
        }
        self.0.store(0, Ordering::Relaxed);
        match value {
            0 => None,
            1 => Some(SystemOcrCompletionStatus::Completed),
            2 => Some(SystemOcrCompletionStatus::Canceled),
            3 => Some(SystemOcrCompletionStatus::Error),
            _ => Some(SystemOcrCompletionStatus::NonTerminal),
        }
    }
}

pub(super) struct SharedSoftwareBitmap {
    pub(super) bitmap: Option<windows::Graphics::Imaging::SoftwareBitmap>,
    pub(super) _ledger: crate::support::perf_probe::ResourceGuard,
}

impl SharedSoftwareBitmap {
    fn bitmap(&self) -> Result<&windows::Graphics::Imaging::SoftwareBitmap, VisionError> {
        self.bitmap
            .as_ref()
            .ok_or_else(|| VisionError::Cleanup("OCR SoftwareBitmap was already closed".into()))
    }

    fn close(&mut self) -> Result<(), String> {
        let Some(bitmap) = self.bitmap.take() else {
            return Ok(());
        };
        bitmap
            .Close()
            .map_err(|error| format!("cannot close OCR SoftwareBitmap: {error}"))
    }
}

impl Drop for SharedSoftwareBitmap {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            crate::support::logging::report_error("windows-vision", error);
        }
    }
}

pub(super) fn run_scan(mut job: ScanJob, shared: &Arc<SharedQueue>, discovery: &DiscoveryHandle) {
    let status = run_scan_inner(&mut job, shared, discovery);
    crate::support::perf_probe::mark("vision_terminal_cleanup");
    job.source.finish(status);
}

fn run_scan_inner(
    job: &mut ScanJob,
    shared: &Arc<SharedQueue>,
    discovery: &DiscoveryHandle,
) -> UiScanStatus {
    let _coordinator_ledger = crate::support::perf_probe::ResourceGuard::new(
        crate::support::perf_probe::ResourceKind::Coordinator,
    );
    let deadline = Instant::now()
        + Duration::from_millis(
            job.request
                .vision
                .request_timeout_ms
                .min(job.request.timeout_ms.max(250))
                .clamp(250, 30_000),
        );
    let bounds = job.request.scan_bounds();
    let geometry = match capture_geometry(bounds) {
        Ok(geometry) => geometry,
        Err(error) => {
            let mut errors = crate::support::errors::ErrorBundle::default();
            errors.push("capture geometry", error);
            if let Some(capture) = job.capture.take() {
                errors.record("capture gate release", capture.release());
            }
            return UiScanStatus::Failed(
                errors
                    .into_result()
                    .err()
                    .unwrap_or_else(|| "capture geometry failed without details".into()),
            );
        }
    };
    let provider_mailbox = Arc::new(ProviderMailbox::new());
    let cancellation = ScanCancellation::new(
        shared,
        job.generation,
        &provider_mailbox,
        Arc::downgrade(&discovery.0),
    );
    {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.active_request_id == Some(job.request.id) {
            state.active_cancellation = Some(cancellation.signal());
        }
    }
    if !generation_is_current(shared, job.generation) {
        cancellation.cancel();
        return UiScanStatus::ContextChanged;
    }
    let discovery_snapshot = if job.request.vision.detect_text {
        match discovery.wait(deadline, || cancellation.is_cancelled()) {
            Some(snapshot) => snapshot,
            None if cancellation.is_cancelled() => return UiScanStatus::ContextChanged,
            None => return UiScanStatus::TimedOut,
        }
    } else {
        Arc::new(OcrDiscoverySnapshot::default())
    };

    let (system_descriptor, wechat_descriptor) =
        OcrExecutionPlan::from_snapshot(&discovery_snapshot, job.request.vision.detect_text)
            .into_descriptors();
    let mut providers = ProviderThreads::new(cancellation.clone(), shared);
    let mut system_input = None;
    let mut wechat_input = None;
    let mut pending_ocr = 0usize;
    if let Some(descriptor) = system_descriptor {
        let layout = SystemOcrLayout::new(geometry);
        let (image_tx, image_rx) = mpsc::sync_channel(layout.max_in_flight.saturating_add(2));
        let completion_tx = image_tx.clone();
        cancellation.register_system_wake(image_tx.clone());
        let (credit_tx, credit_rx) = mpsc::channel();
        cancellation.register_system_credit_wake(credit_tx.clone());
        let result_mailbox = Arc::clone(&provider_mailbox);
        let provider_cancellation = cancellation.clone();
        if providers.spawn("keysteer-system-ocr", move || {
            let _provider_ledger = crate::support::perf_probe::ResourceGuard::new(
                crate::support::perf_probe::ResourceKind::Provider,
            );
            crate::support::perf_probe::mark("system_ocr_started");
            let started = Instant::now();
            let result = recognize_system_provider(
                descriptor,
                image_rx,
                completion_tx,
                credit_tx,
                deadline,
                &provider_cancellation,
                &result_mailbox,
                started,
            );
            let _ = result_mailbox.publish(ProviderEvent::OcrDone {
                provider: "system",
                elapsed: started.elapsed(),
                result,
            });
            crate::support::perf_probe::mark("system_ocr_finished");
        }) {
            system_input = Some(SystemOcrSubmission {
                sender: image_tx,
                credits: credit_rx,
                layout,
                deadline,
            });
            pending_ocr += 1;
        }
    }
    if let Some(descriptor) = wechat_descriptor {
        let (image_tx, image_rx) = mpsc::sync_channel(1);
        cancellation.register_wechat_wake(image_tx.clone());
        let result_mailbox = Arc::clone(&provider_mailbox);
        let provider_cancellation = cancellation.clone();
        let minimum_confidence = job.request.vision.minimum_confidence;
        if providers.spawn("keysteer-wechat-ocr", move || {
            let _provider_ledger = crate::support::perf_probe::ResourceGuard::new(
                crate::support::perf_probe::ResourceKind::Provider,
            );
            crate::support::perf_probe::mark("wechat_ocr_started");
            let started = Instant::now();
            let result = recognize_wechat_provider(
                descriptor,
                image_rx,
                deadline,
                minimum_confidence,
                &provider_cancellation,
                &result_mailbox,
                started,
            );
            let _ = result_mailbox.publish(ProviderEvent::OcrDone {
                provider: "wechat",
                elapsed: started.elapsed(),
                result,
            });
            crate::support::perf_probe::mark("wechat_ocr_finished");
        }) {
            wechat_input = Some(image_tx);
            pending_ocr += 1;
        }
    }
    let fallback_cancelled = Arc::new(AtomicBool::new(false));
    let mut ocr_had_valid_targets = false;
    let mut early_events = ProviderEvents::new();

    let Some(mut capture_lease) = job.capture.take() else {
        return UiScanStatus::Failed("visual capture lease was not created".into());
    };
    // Geometry, the thread-bound DIB and the WinRT apartment are prepared
    // while the renderer is independently hiding its tree. No desktop pixels
    // are read before the hidden ACK.
    let mut prepared_capture = match crate::platform::windows::native::PreparedCapture::new(
        geometry.width,
        geometry.height,
    ) {
        Ok(capture) => capture,
        Err(error) => {
            let mut errors = crate::support::errors::ErrorBundle::default();
            errors.push("capture DIB preparation", error);
            errors.record("capture gate release", capture_lease.release());
            return UiScanStatus::Failed(
                errors
                    .into_result()
                    .err()
                    .unwrap_or_else(|| "capture DIB preparation failed without details".into()),
            );
        }
    };
    let gdi_ledger = crate::support::perf_probe::ResourceGuard::new(
        crate::support::perf_probe::ResourceKind::GdiSurface,
    );
    crate::support::perf_probe::mark("capture_dib_prepared");
    let mut bitmap_factory_error = None;
    let bitmap_apartment = if system_input.is_none() && wechat_input.is_none() {
        None
    } else {
        match crate::platform::windows::native::ComApartment::initialise() {
            Ok(apartment) => Some(apartment),
            Err(error) => {
                bitmap_factory_error = Some(error);
                None
            }
        }
    };
    let bitmap_factory = bitmap_apartment.as_ref().and_then(|_| {
        match crate::platform::windows::native::SoftwareBitmapFactory::load() {
            Ok(factory) => Some(factory),
            Err(error) => {
                bitmap_factory_error = Some(error);
                None
            }
        }
    });
    if let Err(error) =
        capture_lease.wait_hidden(deadline, || !generation_is_current(shared, job.generation))
    {
        let current = context_is_current(shared, job.generation, &job.request);
        if let Err(release_error) = capture_lease.release() {
            crate::support::logging::report_error("windows-overlay", release_error);
        }
        if !current {
            return UiScanStatus::ContextChanged;
        }
        if Instant::now() >= deadline {
            return UiScanStatus::TimedOut;
        }
        return UiScanStatus::Failed(error);
    }
    if !context_is_current(shared, job.generation, &job.request) {
        if let Err(error) = capture_lease.release() {
            crate::support::logging::report_error("windows-overlay", error);
        }
        return UiScanStatus::ContextChanged;
    }
    crate::support::perf_probe::mark("capture_hidden_ack");
    let mut capture_lease = Some(capture_lease);
    let mut context_changed_during_capture = false;
    let captured = prepared_capture.capture_with(
        bounds.x.floor() as i32,
        bounds.y.floor() as i32,
        bounds.width.ceil() as i32,
        bounds.height.ceil() as i32,
        |pixels, width, height| {
            crate::support::perf_probe::mark("capture_gdi_ready");
            if width != geometry.width || height != geometry.height {
                return Err("native capture dimensions changed unexpectedly".into());
            }
            if !context_is_current(shared, job.generation, &job.request) {
                context_changed_during_capture = true;
                return Err("visual capture context changed".into());
            }
            // The DIB already contains a stable desktop frame. Release the
            // generation gate before constructing OCR/fallback artifacts so a
            // deferred UIA frame can be shown without waiting for pixel work.
            if let Some(lease) = capture_lease.take() {
                lease.release()?;
            }
            if let Some(input) = system_input.take() {
                if let Some(factory) = bitmap_factory.as_ref() {
                    if let Err(error) =
                        stream_system_ocr_tiles(pixels, geometry, factory, &input, |index| {
                            if index == Some(0) && wechat_input.is_some() {
                                submit_wechat_full_frame(
                                    &mut wechat_input,
                                    pixels,
                                    geometry,
                                    bitmap_factory.as_ref(),
                                    bitmap_factory_error.as_deref(),
                                );
                            }
                            if !drain_early_ocr_events(
                                &provider_mailbox,
                                &job.source,
                                &fallback_cancelled,
                                &mut ocr_had_valid_targets,
                                &mut early_events,
                                || context_is_current(shared, job.generation, &job.request),
                            ) {
                                context_changed_during_capture = true;
                                return Err("visual capture context changed".into());
                            }
                            Ok(())
                        })
                    {
                        let _ = input.sender.send(SystemOcrInput::Failed(error));
                        let _ = input.sender.send(SystemOcrInput::Done);
                    }
                } else {
                    let _ = input.sender.send(SystemOcrInput::Failed(
                        bitmap_factory_error.clone().unwrap_or_else(|| {
                            "cannot create the COM apartment for system OCR tiles".into()
                        }),
                    ));
                    let _ = input.sender.send(SystemOcrInput::Done);
                }
            }
            if wechat_input.is_some() {
                submit_wechat_full_frame(
                    &mut wechat_input,
                    pixels,
                    geometry,
                    bitmap_factory.as_ref(),
                    bitmap_factory_error.as_deref(),
                );
            }
            if !drain_early_ocr_events(
                &provider_mailbox,
                &job.source,
                &fallback_cancelled,
                &mut ocr_had_valid_targets,
                &mut early_events,
                || context_is_current(shared, job.generation, &job.request),
            ) {
                context_changed_during_capture = true;
                return Err("visual capture context changed".into());
            }
            let fallback = if job.request.vision.detect_rectangles && !ocr_had_valid_targets {
                fallback_input_from_bgra_with_progress(pixels, geometry, || {
                    if !drain_early_ocr_events(
                        &provider_mailbox,
                        &job.source,
                        &fallback_cancelled,
                        &mut ocr_had_valid_targets,
                        &mut early_events,
                        || context_is_current(shared, job.generation, &job.request),
                    ) {
                        context_changed_during_capture = true;
                        return Ok(true);
                    }
                    Ok(ocr_had_valid_targets || cancellation.is_cancelled())
                })?
            } else {
                None
            };
            Ok(fallback)
        },
    );
    drop(prepared_capture);
    drop(gdi_ledger);
    if let Some(lease) = capture_lease.take()
        && let Err(error) = lease.release()
    {
        crate::support::logging::report_error("windows-overlay", error);
    }
    let fallback_input = match captured {
        Ok(artifact) => artifact,
        Err(_) if context_changed_during_capture => return UiScanStatus::ContextChanged,
        Err(error) => return UiScanStatus::Failed(error),
    };
    if !context_is_current(shared, job.generation, &job.request) {
        return UiScanStatus::ContextChanged;
    }

    drop(system_input);
    drop(wechat_input);

    let fallback_pending = if let Some(fallback_input) = fallback_input {
        let result_mailbox = Arc::clone(&provider_mailbox);
        let options = job.request.vision.clone();
        let provider_cancellation = cancellation.clone();
        let fallback_cancelled = Arc::clone(&fallback_cancelled);
        providers.spawn("keysteer-vision-fallback", move || {
            let _provider_ledger = crate::support::perf_probe::ResourceGuard::new(
                crate::support::perf_probe::ResourceKind::Provider,
            );
            crate::support::perf_probe::mark("vision_fallback_started");
            let mut scratch = FallbackScratch::default();
            let targets = detect_regions(&fallback_input, &options, &mut scratch, || {
                provider_cancellation.is_cancelled() || fallback_cancelled.load(Ordering::Acquire)
            });
            let _ = send_fallback_batches(&result_mailbox, targets);
            crate::support::perf_probe::mark("vision_fallback_finished");
        })
    } else {
        false
    };
    let mut fallback = Vec::new();
    let mut fallback_done = !fallback_pending;
    let mut timed_out = false;
    let mut context_changed = false;
    let mut cleanup_errors = crate::support::errors::ErrorBundle::default();
    while pending_ocr != 0 || !fallback_done {
        if !generation_is_current(shared, job.generation) {
            cancellation.cancel();
            context_changed = true;
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            cancellation.cancel();
            break;
        }
        let mut events = std::mem::take(&mut early_events);
        if events.is_empty() {
            match provider_mailbox.wait_until_ready(deadline) {
                Ok(()) => {}
                Err(VisionError::TimedOut) => {
                    timed_out = true;
                    cancellation.cancel();
                    break;
                }
                Err(VisionError::Cancelled) if cancellation.is_cancelled() => break,
                Err(error) => {
                    cleanup_errors.push("provider mailbox", error.to_string());
                    break;
                }
            }
        }
        provider_mailbox.drain_into(&mut events);
        let mut ocr_ready: SmallVec<[ReadyOcrBatch; 2]> = SmallVec::new();
        let mut ocr_done: SmallVec<[CompletedOcr; 2]> = SmallVec::new();
        for event in events {
            match event {
                ProviderEvent::FallbackBatch(mut targets) => fallback.append(&mut targets),
                ProviderEvent::FallbackDone => fallback_done = true,
                ProviderEvent::OcrBatch {
                    provider,
                    elapsed,
                    targets,
                } => ocr_ready.push((provider, elapsed, targets)),
                ProviderEvent::OcrDone {
                    provider,
                    elapsed,
                    result,
                } => ocr_done.push((provider, elapsed, result)),
            }
        }
        ocr_ready.sort_by(|a, b| compare_ready(a.2.len(), a.1, b.2.len(), b.1));
        for (provider, _elapsed, targets) in ocr_ready {
            if !context_is_current(shared, job.generation, &job.request) {
                cancellation.cancel();
                context_changed = true;
                break;
            }
            let count = targets.len();
            if count != 0 {
                ocr_had_valid_targets = true;
                fallback_cancelled.store(true, Ordering::Release);
            }
            let accepted = job.source.push(targets);
            if accepted != 0 {
                crate::support::perf_probe::mark("vision_targets_accepted");
            }
            crate::log_info!(
                "windows-vision",
                "{provider} OCR streamed {count} valid targets ({accepted} new)"
            );
        }
        for (provider, elapsed, result) in ocr_done {
            pending_ocr = pending_ocr.saturating_sub(1);
            match result {
                Ok(count) => {
                    crate::log_info!(
                        "windows-vision",
                        "{provider} OCR completed in {elapsed:?} with {count} valid targets"
                    );
                }
                Err(error) if error.is_control_flow() => {}
                Err(error @ VisionError::Unavailable(_)) => crate::report_warning!(
                    "windows-vision",
                    "{provider} OCR failed after {elapsed:?}: {error}"
                ),
                Err(error) => {
                    if matches!(error, VisionError::Cleanup(_)) {
                        cleanup_errors.push(
                            format!("{provider} OCR cleanup after {elapsed:?}"),
                            error.to_string(),
                        );
                    } else {
                        // Operational provider failures can be masked by the
                        // other OCR or the Rust fallback, so this coordinator
                        // is their final reporting boundary.
                        crate::report_error!(
                            "windows-vision",
                            "{provider} OCR failed after {elapsed:?}: {error}"
                        );
                    }
                }
            }
        }
    }

    if !context_changed
        && should_publish_fallback(ocr_had_valid_targets, job.request.vision.detect_rectangles)
        && fallback_done
        && !fallback.is_empty()
    {
        if context_is_current(shared, job.generation, &job.request) {
            job.source.push(fallback);
        } else {
            context_changed = true;
        }
    }
    cleanup_errors.record(
        "provider join",
        providers.join_all(Instant::now() + PROVIDER_STOP_TIMEOUT),
    );
    drop(bitmap_apartment);
    let cleanup_result = cleanup_errors.into_result();
    if context_changed || !context_is_current(shared, job.generation, &job.request) {
        if let Err(error) = cleanup_result {
            crate::support::logging::report_error("windows-vision", error);
        }
        UiScanStatus::ContextChanged
    } else if timed_out {
        if let Err(error) = cleanup_result {
            crate::support::logging::report_error("windows-vision", error);
        }
        UiScanStatus::TimedOut
    } else if let Err(error) = cleanup_result {
        UiScanStatus::Failed(error)
    } else {
        UiScanStatus::Success
    }
}

pub(super) fn should_publish_fallback(
    ocr_had_valid_targets: bool,
    rectangles_enabled: bool,
) -> bool {
    rectangles_enabled && !ocr_had_valid_targets
}

fn wait_provider_image(
    receiver: mpsc::Receiver<WechatInput>,
    deadline: Instant,
    cancellation: &ScanCancellation,
) -> Result<WechatFullFrame, VisionError> {
    if cancellation.is_cancelled() {
        return Err(VisionError::Cancelled);
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(VisionError::TimedOut);
    }
    match receiver.recv_timeout(remaining) {
        Ok(WechatInput::Frame(image)) => Ok(image),
        Ok(WechatInput::CancelWake) => Err(VisionError::Cancelled),
        Ok(WechatInput::Failed(error)) => Err(VisionError::Operational(error)),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(VisionError::TimedOut),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(VisionError::Cancelled),
    }
}

#[allow(clippy::too_many_arguments)]
fn recognize_system_provider(
    descriptor: Arc<SystemOcrDescriptor>,
    receiver: mpsc::Receiver<SystemOcrInput>,
    completion_sender: mpsc::SyncSender<SystemOcrInput>,
    credit_sender: mpsc::Sender<()>,
    deadline: Instant,
    cancellation: &ScanCancellation,
    result_mailbox: &ProviderMailbox,
    started: Instant,
) -> Result<usize, VisionError> {
    let _apartment = crate::platform::windows::native::ComApartment::initialise()
        .map_err(VisionError::Operational)?;
    let factory = crate::platform::windows::native::SystemOcrFactory::load()
        .map_err(VisionError::Unavailable)?;
    // Preserve the existing overlap between OCR cold start and the overlay
    // hide/capture barrier. The remaining engines are created as tiles arrive.
    let mut first_engine = Some(factory.create_engine().map_err(VisionError::Unavailable)?);
    let mut expected_tiles = None;
    let mut active: Vec<Option<ActiveSystemOcrTile>> = Vec::new();
    let mut pending = 0usize;
    let mut accepted = 0usize;
    let mut operational_failures = Vec::new();
    let mut cleanup_failures = Vec::new();
    loop {
        if cancellation.is_cancelled() {
            cancel_active_system_tiles(&mut active, &receiver, deadline, &mut cleanup_failures);
            return if cleanup_failures.is_empty() {
                Err(VisionError::Cancelled)
            } else {
                Err(VisionError::Cleanup(cleanup_failures.join("; ")))
            };
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            cancel_active_system_tiles(&mut active, &receiver, deadline, &mut cleanup_failures);
            return if cleanup_failures.is_empty() {
                Err(VisionError::TimedOut)
            } else {
                Err(VisionError::Cleanup(cleanup_failures.join("; ")))
            };
        }
        let input = match receiver.recv_timeout(remaining) {
            Ok(input) => input,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                cancel_active_system_tiles(&mut active, &receiver, deadline, &mut cleanup_failures);
                return if cleanup_failures.is_empty() {
                    Err(VisionError::TimedOut)
                } else {
                    Err(VisionError::Cleanup(cleanup_failures.join("; ")))
                };
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                operational_failures.push("system OCR tile stream disconnected".into());
                break;
            }
        };
        match input {
            SystemOcrInput::Begin { tile_count } => {
                if expected_tiles.replace(tile_count).is_some() {
                    operational_failures.push("system OCR received a duplicate tile header".into());
                    continue;
                }
                active.reserve(tile_count);
            }
            SystemOcrInput::Tile(mut tile) => {
                if expected_tiles.is_none() {
                    operational_failures
                        .push("system OCR received a tile before its header".into());
                    if let Err(error) = tile.bitmap.close() {
                        cleanup_failures.push(error);
                    }
                    let _ = credit_sender.send(());
                    continue;
                }
                let index = active.len();
                if accepted >= MAX_OCR_TARGETS {
                    if let Err(error) = tile.bitmap.close() {
                        cleanup_failures.push(error);
                    }
                    active.push(None);
                    let _ = credit_sender.send(());
                    continue;
                }
                if tile.geometry.width > descriptor.maximum_dimension
                    || tile.geometry.height > descriptor.maximum_dimension
                {
                    operational_failures.push(format!(
                        "system OCR tile {}x{} exceeds the OCR limit {}",
                        tile.geometry.width, tile.geometry.height, descriptor.maximum_dimension
                    ));
                    if let Err(error) = tile.bitmap.close() {
                        cleanup_failures.push(error);
                    }
                    active.push(None);
                    let _ = credit_sender.send(());
                    continue;
                }
                let engine = match first_engine
                    .take()
                    .map_or_else(|| factory.create_engine(), Ok)
                {
                    Ok(engine) => engine,
                    Err(error) => {
                        operational_failures.push(error);
                        if let Err(error) = tile.bitmap.close() {
                            cleanup_failures.push(error);
                        }
                        active.push(None);
                        let _ = credit_sender.send(());
                        continue;
                    }
                };
                let (operation, completion) = match tile.bitmap.bitmap().and_then(|bitmap| {
                    OcrOperationGuard::start_notified(
                        &engine,
                        bitmap,
                        index,
                        completion_sender.clone(),
                    )
                    .map_err(VisionError::Operational)
                }) {
                    Ok(operation) => operation,
                    Err(error) => {
                        operational_failures.push(error.to_string());
                        if let Err(error) = tile.bitmap.close() {
                            cleanup_failures.push(error);
                        }
                        active.push(None);
                        let _ = credit_sender.send(());
                        continue;
                    }
                };
                active.push(Some(ActiveSystemOcrTile {
                    _engine: engine,
                    operation,
                    completion,
                    tile,
                }));
                pending += 1;
            }
            SystemOcrInput::CompletionWake | SystemOcrInput::CancelWake => {}
            SystemOcrInput::Failed(error) => operational_failures.push(error),
            SystemOcrInput::Done => break,
        }
        drain_completed_system_tiles(
            &mut active,
            &mut pending,
            &mut accepted,
            result_mailbox,
            started,
            &credit_sender,
            &mut operational_failures,
            &mut cleanup_failures,
        )?;
    }
    drop(first_engine);
    if let Some(expected) = expected_tiles
        && active.len() != expected
    {
        operational_failures.push(format!(
            "system OCR expected {expected} tiles but received {}",
            active.len()
        ));
    }

    while pending != 0 && accepted < MAX_OCR_TARGETS {
        drain_completed_system_tiles(
            &mut active,
            &mut pending,
            &mut accepted,
            result_mailbox,
            started,
            &credit_sender,
            &mut operational_failures,
            &mut cleanup_failures,
        )?;
        if pending == 0 || accepted >= MAX_OCR_TARGETS {
            break;
        }
        if cancellation.is_cancelled() || Instant::now() >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let completion = receiver.recv_timeout(remaining);
        match completion {
            Ok(SystemOcrInput::CompletionWake | SystemOcrInput::CancelWake) => {}
            Ok(SystemOcrInput::Failed(error)) => {
                operational_failures.push(error);
            }
            Ok(SystemOcrInput::Begin { .. } | SystemOcrInput::Tile(_) | SystemOcrInput::Done) => {
                operational_failures.push("system OCR received input after tile stream end".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                operational_failures.push("system OCR completion channel disconnected".into());
                break;
            }
        };
    }
    cancel_active_system_tiles(&mut active, &receiver, deadline, &mut cleanup_failures);
    if !cleanup_failures.is_empty() {
        Err(VisionError::Cleanup(cleanup_failures.join("; ")))
    } else if cancellation.is_cancelled() {
        Err(VisionError::Cancelled)
    } else if Instant::now() >= deadline {
        Err(VisionError::TimedOut)
    } else if !operational_failures.is_empty() {
        Err(VisionError::Operational(operational_failures.join("; ")))
    } else {
        Ok(accepted)
    }
}

struct ActiveSystemOcrTile {
    _engine: OcrEngine,
    operation: OcrOperationGuard,
    completion: Arc<SystemOcrCompletion>,
    tile: SystemOcrTile,
}

#[allow(clippy::too_many_arguments)]
fn drain_completed_system_tiles(
    active: &mut [Option<ActiveSystemOcrTile>],
    pending: &mut usize,
    accepted: &mut usize,
    result_mailbox: &ProviderMailbox,
    started: Instant,
    credit_sender: &mpsc::Sender<()>,
    operational_failures: &mut Vec<String>,
    cleanup_failures: &mut Vec<String>,
) -> Result<(), VisionError> {
    for index in 0..active.len() {
        let Some(status) = active[index]
            .as_ref()
            .and_then(|tile| tile.completion.take())
        else {
            continue;
        };
        complete_system_tile(
            (index, status),
            active,
            pending,
            accepted,
            result_mailbox,
            started,
            credit_sender,
            operational_failures,
            cleanup_failures,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn complete_system_tile(
    (index, status): (usize, SystemOcrCompletionStatus),
    active: &mut [Option<ActiveSystemOcrTile>],
    pending: &mut usize,
    accepted: &mut usize,
    result_mailbox: &ProviderMailbox,
    started: Instant,
    credit_sender: &mpsc::Sender<()>,
    operational_failures: &mut Vec<String>,
    cleanup_failures: &mut Vec<String>,
) -> Result<(), VisionError> {
    let Some(slot) = active.get_mut(index) else {
        operational_failures.push(format!(
            "system OCR returned an invalid tile completion index {index}"
        ));
        return Ok(());
    };
    let Some(mut completed) = slot.take() else {
        operational_failures.push(format!(
            "system OCR returned a duplicate tile completion index {index}"
        ));
        return Ok(());
    };
    *pending = pending.saturating_sub(1);
    let mut fatal = None;
    match completed.operation.complete(status) {
        Ok(result) => match stream_system_targets_from_result(
            &result,
            completed.tile.geometry,
            completed.tile.core_bounds,
            MAX_OCR_TARGETS.saturating_sub(*accepted),
            result_mailbox,
            started,
        ) {
            Ok(count) => *accepted += count,
            Err(VisionError::Operational(error)) => operational_failures.push(error),
            Err(error) => fatal = Some(error),
        },
        Err(error) => operational_failures.push(format!("system OCR tile {index}: {error}")),
    }
    if let Err(error) = completed.tile.bitmap.close() {
        cleanup_failures.push(error);
    }
    let _ = credit_sender.send(());
    fatal.map_or(Ok(()), Err)
}

fn cancel_active_system_tiles(
    active: &mut [Option<ActiveSystemOcrTile>],
    completions: &mpsc::Receiver<SystemOcrInput>,
    deadline: Instant,
    cleanup_failures: &mut Vec<String>,
) {
    let mut remaining = 0usize;
    for tile in active.iter().flatten() {
        remaining += 1;
        if let Err(error) = tile.operation.request_cancel() {
            cleanup_failures.push(error);
        }
    }
    while remaining != 0 {
        for slot in active.iter_mut() {
            let Some(status) = slot.as_ref().and_then(|tile| tile.completion.take()) else {
                continue;
            };
            let Some(mut tile) = slot.take() else {
                continue;
            };
            remaining = remaining.saturating_sub(1);
            let close = match status {
                SystemOcrCompletionStatus::Completed => tile.operation.complete(status).map(drop),
                SystemOcrCompletionStatus::Canceled | SystemOcrCompletionStatus::Error => {
                    tile.operation.close_terminal()
                }
                SystemOcrCompletionStatus::NonTerminal => retain_nonterminal_system_ocr_owner(
                    "system OCR completion callback reported a non-terminal status during cancellation",
                ),
            };
            if let Err(error) = close {
                cleanup_failures.push(error);
            }
            if let Err(error) = tile.tile.bitmap.close() {
                cleanup_failures.push(error);
            }
        }
        if remaining == 0 {
            break;
        }
        let completion = wait_system_ocr_cancellation_event(completions, deadline);
        match completion {
            Ok(SystemOcrInput::CompletionWake | SystemOcrInput::CancelWake) => {}
            Ok(_) => continue,
            Err(_) => retain_nonterminal_system_ocr_owner(
                "system OCR completion channel closed while an operation was still active",
            ),
        }
    }
}

pub(super) fn wait_system_ocr_cancellation_event(
    completions: &mpsc::Receiver<SystemOcrInput>,
    deadline: Instant,
) -> Result<SystemOcrInput, mpsc::RecvError> {
    let remaining_time = deadline.saturating_duration_since(Instant::now());
    if remaining_time.is_zero() {
        // Keep the complete tile owner (operation, engine, bitmap and this COM
        // apartment) on the provider thread until WinRT reports a terminal
        // state. The coordinator has its own bounded join; if a native
        // operation never calls back, that WorkerJoin moves to the explicit
        // provider quarantine instead of dropping live objects.
        return completions.recv();
    }
    match completions.recv_timeout(remaining_time) {
        Ok(input) => Ok(input),
        Err(mpsc::RecvTimeoutError::Timeout) => completions.recv(),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(mpsc::RecvError),
    }
}

#[cold]
#[inline(never)]
pub(super) fn retain_nonterminal_system_ocr_owner(reason: &str) -> ! {
    crate::support::logging::report_error(
        "windows-vision",
        format!("{reason}; retaining its complete provider owner in quarantine"),
    );
    loop {
        std::thread::park();
    }
}

fn recognize_wechat_provider(
    descriptor: Arc<WechatDescriptor>,
    receiver: mpsc::Receiver<WechatInput>,
    deadline: Instant,
    minimum_confidence: f64,
    cancellation: &ScanCancellation,
    result_mailbox: &ProviderMailbox,
    started: Instant,
) -> Result<usize, VisionError> {
    let _apartment = crate::platform::windows::native::ComApartment::initialise()
        .map_err(VisionError::Operational)?;
    let mut provider = WechatOcr::start(&descriptor, deadline, &|| cancellation.is_cancelled())
        .map_err(VisionError::Unavailable)?;
    let result = wait_provider_image(receiver, deadline, cancellation).and_then(|mut input| {
        let geometry = input.geometry;
        let bitmap = input.take_bitmap()?;
        provider
            .recognize(
                geometry,
                bitmap,
                deadline.saturating_duration_since(Instant::now()),
                minimum_confidence,
                || cancellation.is_cancelled(),
                |targets| {
                    send_ocr_batches(result_mailbox, "wechat", started, targets)
                        .map_err(|error| error.to_string())
                },
            )
            .map_err(|error| {
                if cancellation.is_cancelled() {
                    VisionError::Cancelled
                } else if Instant::now() >= deadline {
                    VisionError::TimedOut
                } else {
                    VisionError::Operational(error)
                }
            })
    });
    let cleanup = provider.shutdown();
    match (result, cleanup) {
        (Ok(targets), Ok(())) => Ok(targets),
        (Err(_), Ok(())) if cancellation.is_cancelled() => Err(VisionError::Cancelled),
        (Err(_), Ok(())) if Instant::now() >= deadline => Err(VisionError::TimedOut),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(VisionError::Cleanup(error)),
        (Err(error), Err(cleanup)) => {
            Err(VisionError::Cleanup(format!("{error}; cleanup: {cleanup}")))
        }
    }
}
