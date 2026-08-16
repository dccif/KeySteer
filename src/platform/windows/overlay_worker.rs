#![forbid(unsafe_code)]

//! Non-blocking latest-frame submission for the Windows overlay.
//!
//! The engine/input thread only replaces one pending frame. A dedicated
//! normal-priority thread owns every HWND and GDI resource, so a dense scene
//! cannot delay the synchronous hook disposition or native click injection.

use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::api::geometry::{Point, Rect};
use crate::api::overlay::OverlayScene;
use crate::app::worker::WorkerJoin;

use super::{EventSender, gpu_overlay::GpuOverlay, native, overlay};

const DEVICE_FAILURE_WINDOW: Duration = Duration::from_secs(60);
const RENDER_WAKE_MESSAGE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x50;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

struct Frame {
    scene: Arc<OverlayScene>,
    area: Rect,
    scale: f64,
}

#[derive(Clone, Copy)]
struct Positions {
    cursor: Option<Point>,
    indicator: Option<Point>,
}

enum Control {
    BeginCapture(u64),
    Dismiss,
    Shutdown(SyncSender<Result<(), String>>),
}

struct CaptureGate {
    generation: u64,
    ready: Option<SyncSender<Result<(), String>>>,
    deferred_frame: Option<Frame>,
    deferred_positions: Option<Positions>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlayPhase {
    Normal,
    HidingForCapture(u64),
    HiddenForCapture(u64),
    Releasing(u64),
    Stopping,
}

struct State {
    latest: Option<Frame>,
    positions: Option<Positions>,
    control: Option<Control>,
    capture: Option<CaptureGate>,
    phase: OverlayPhase,
    alive: bool,
    wake_pending: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            latest: None,
            positions: None,
            control: None,
            capture: None,
            phase: OverlayPhase::Normal,
            alive: true,
            wake_pending: false,
        }
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
}

fn mark_wake_pending(state: &mut State) -> Result<bool, String> {
    if !state.alive {
        return Err("Windows overlay renderer has already stopped".into());
    }
    if state.wake_pending
        || (state.control.is_none() && state.latest.is_none() && state.positions.is_none())
    {
        return Ok(false);
    }
    state.wake_pending = true;
    Ok(true)
}

pub(super) struct OverlayWorker {
    shared: Arc<Shared>,
    thread_id: u32,
    worker: Option<WorkerJoin>,
}

/// A generation-scoped permit which keeps overlay frames off-screen until the
/// vision worker has copied the desktop pixels. Dropping a stale lease can
/// never release a newer generation.
#[must_use = "the capture gate remains hidden until this generation lease is released"]
pub(super) struct CaptureLease {
    shared: Arc<Shared>,
    thread_id: u32,
    generation: u64,
    ready: Option<Receiver<Result<(), String>>>,
    released: bool,
}

impl CaptureLease {
    pub(super) fn wait_hidden(
        &mut self,
        deadline: Instant,
        mut canceled: impl FnMut() -> bool,
    ) -> Result<(), String> {
        let ready = self
            .ready
            .take()
            .ok_or("capture lease hidden acknowledgement was already consumed")?;
        loop {
            if canceled() {
                return Err("capture lease was canceled".into());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err("timed out waiting for the overlay capture barrier".into());
            }
            match ready.recv_timeout((deadline - now).min(Duration::from_millis(10))) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("overlay renderer stopped before hiding for capture".into());
                }
            }
        }
    }

    /// Release the gate and display only the latest frame deferred while the
    /// screenshot was pending.
    pub(super) fn release(mut self) -> Result<(), String> {
        self.release_inner(true)
    }

    fn release_inner(&mut self, show_deferred: bool) -> Result<(), String> {
        if self.released {
            return Ok(());
        }
        self.released = true;
        let should_wake = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(gate) = state
                .capture
                .take_if(|gate| gate.generation == self.generation)
            else {
                return Ok(());
            };
            state.phase = OverlayPhase::Releasing(self.generation);
            if matches!(state.control, Some(Control::BeginCapture(generation)) if generation == self.generation)
            {
                state.control = None;
            }
            if show_deferred && let Some(frame) = gate.deferred_frame {
                state.latest = Some(frame);
                state.positions = gate.deferred_positions;
            } else {
                // A position update cannot be rendered without the complete
                // frame it describes. This is expected when the pointer moves
                // while a capture gate is hidden and no provider has produced
                // a new frame yet.
                state.latest = None;
                state.positions = None;
            }
            state.phase = OverlayPhase::Normal;
            mark_wake_pending(&mut state)?
        };
        if should_wake
            && let Err(error) = native::post_thread_wake(self.thread_id, RENDER_WAKE_MESSAGE)
        {
            let error = format!("cannot wake Windows overlay renderer: {error}");
            fail_renderer(&self.shared, &error);
            return Err(error);
        }
        Ok(())
    }
}

impl Drop for CaptureLease {
    fn drop(&mut self) {
        if let Err(error) = self.release_inner(false) {
            crate::report_error!("windows-overlay", "{error}");
        }
    }
}

impl OverlayWorker {
    pub(super) fn start(events: EventSender) -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let mut worker = WorkerJoin::spawn(
            "Windows overlay renderer",
            std::thread::Builder::new().name("keysteer-overlay-render".into()),
            move || render_loop(&thread_shared, &events, ready_tx),
        )?;
        let thread_id = match worker.wait_ready(&ready_rx, STOP_TIMEOUT) {
            Ok(thread_id) => thread_id,
            Err(error) => {
                if let Err(cleanup) = worker.join_timeout(STOP_TIMEOUT) {
                    crate::report_error!(
                        "windows-overlay",
                        "overlay startup cleanup failed: {cleanup}"
                    );
                }
                return Err(error);
            }
        };
        Ok(Self {
            shared,
            thread_id,
            worker: Some(worker),
        })
    }

    /// Replace the pending frame and return immediately. Superseded frames are
    /// dropped before any native rendering work begins.
    pub(super) fn present(
        &self,
        scene: Arc<OverlayScene>,
        area: Rect,
        scale: f64,
    ) -> Result<(), String> {
        let should_wake = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            ensure_renderer_alive(&state)?;
            let frame = Frame { scene, area, scale };
            if let Some(gate) = state.capture.as_mut() {
                gate.deferred_frame = Some(frame);
                gate.deferred_positions = None;
                return Ok(());
            }
            state.latest = Some(frame);
            state.positions = None;
            mark_wake_pending(&mut state)?
        };
        self.post_wake(should_wake)
    }

    /// Coalesce dynamic positions independently from complete frames.
    pub(super) fn update_positions(
        &self,
        cursor: Option<Point>,
        indicator: Option<Point>,
    ) -> Result<(), String> {
        let should_wake = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            ensure_renderer_alive(&state)?;
            let positions = Positions { cursor, indicator };
            if let Some(gate) = state.capture.as_mut() {
                // The next complete frame is authoritative and already uses
                // the Engine's current pointer. Keeping a position without a
                // base frame would release position-only work after capture.
                if gate.deferred_frame.is_some() {
                    gate.deferred_positions = Some(positions);
                }
                return Ok(());
            }
            state.positions = Some(positions);
            mark_wake_pending(&mut state)?
        };
        self.post_wake(should_wake)
    }

    pub(super) fn dismiss(&self) -> Result<(), String> {
        if self.worker.is_none() {
            return Ok(());
        }
        let should_wake = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            ensure_renderer_alive(&state)?;
            state.latest = None;
            state.positions = None;
            cancel_capture_locked(&mut state, "overlay was dismissed");
            state.phase = OverlayPhase::Normal;
            state.control = Some(Control::Dismiss);
            mark_wake_pending(&mut state)?
        };
        self.post_wake(should_wake)
    }

    /// Begin hiding the overlay for a single scan generation. This method only
    /// enqueues renderer work and never waits for DWM on the engine thread.
    pub(super) fn begin_capture(&self, generation: u64) -> Result<CaptureLease, String> {
        if self.worker.is_none() {
            return Err("Windows overlay renderer is not running".into());
        }
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        crate::app::perf_probe::mark("capture_gate_started");
        let should_wake = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !state.alive || matches!(state.control, Some(Control::Shutdown(_))) {
                return Err("Windows overlay renderer is stopping".into());
            }
            if let Some(mut previous) = state.capture.take()
                && let Some(reply) = previous.ready.take()
            {
                let _ = reply.send(Err("capture generation was superseded".into()));
            }
            state.latest = None;
            state.positions = None;
            state.capture = Some(CaptureGate {
                generation,
                ready: Some(ready_tx),
                deferred_frame: None,
                deferred_positions: None,
            });
            state.phase = OverlayPhase::HidingForCapture(generation);
            state.control = Some(Control::BeginCapture(generation));
            mark_wake_pending(&mut state)?
        };
        if let Err(error) = self.post_wake(should_wake) {
            cancel_capture(&self.shared, generation, &error);
            return Err(error);
        }
        Ok(CaptureLease {
            shared: Arc::clone(&self.shared),
            thread_id: self.thread_id,
            generation,
            ready: Some(ready_rx),
            released: false,
        })
    }

    fn control(
        &self,
        make: impl FnOnce(SyncSender<Result<(), String>>) -> Control,
    ) -> Result<(), String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let should_wake = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            ensure_renderer_alive(&state)?;
            state.latest = None;
            state.positions = None;
            cancel_capture_locked(&mut state, "overlay was dismissed");
            let control = make(reply_tx);
            state.phase = if matches!(control, Control::Shutdown(_)) {
                OverlayPhase::Stopping
            } else {
                OverlayPhase::Normal
            };
            state.control = Some(control);
            mark_wake_pending(&mut state)?
        };
        self.post_wake(should_wake)?;
        reply_rx
            .recv_timeout(CONTROL_TIMEOUT)
            .map_err(|error| format!("Windows overlay renderer did not reply: {error}"))?
    }

    fn post_wake(&self, should_wake: bool) -> Result<(), String> {
        if !should_wake {
            return Ok(());
        }
        if let Err(error) = native::post_thread_wake(self.thread_id, RENDER_WAKE_MESSAGE) {
            let error = format!("cannot wake Windows overlay renderer: {error}");
            fail_renderer(&self.shared, &error);
            return Err(error);
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        let result = if self.worker.is_some() {
            self.control(Control::Shutdown)
        } else {
            Ok(())
        };
        let join_result = self
            .worker
            .as_mut()
            .map_or(Ok(()), |worker| worker.join_timeout(STOP_TIMEOUT));
        if join_result.is_ok() {
            self.worker.take();
            self.thread_id = 0;
        }
        if result.is_err() { result } else { join_result }
    }

    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        self.stop()
    }
}

impl Drop for OverlayWorker {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            crate::app::logging::report_error("windows-overlay", error);
        }
    }
}

fn render_loop(shared: &Shared, events: &EventSender, ready: SyncSender<u32>) {
    let thread_id = native::prepare_thread_message_queue();
    if ready.send(thread_id).is_err() {
        mark_renderer_stopped(shared, "overlay renderer startup was abandoned");
        return;
    }
    let (mut renderer, startup_notice) = AdaptiveRenderer::new();
    let mut dpi_cache = overlay::DpiSceneCache::default();
    if let Some(notice) = startup_notice {
        warn(events, notice);
    }
    let mut scale = 1.0;
    loop {
        let (control, frame, positions) = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.wake_pending = false;
            (
                state.control.take(),
                state.latest.take(),
                state.positions.take(),
            )
        };

        if control.is_none() && frame.is_none() && positions.is_none() {
            match native::wait_and_dispatch_window_message() {
                Ok(true) => continue,
                Ok(false) => {
                    mark_renderer_stopped(shared, "overlay renderer message loop stopped");
                    return;
                }
                Err(error) => {
                    crate::report_error!(
                        "windows-overlay",
                        "Windows overlay message loop failed: {error}"
                    );
                    mark_renderer_stopped(shared, "overlay renderer message loop failed");
                    return;
                }
            }
        }

        if let Some(control) = control {
            let capture_generation = match control {
                Control::BeginCapture(generation) => Some(generation),
                Control::Dismiss | Control::Shutdown(_) => None,
            };
            let result = if capture_generation.is_some() {
                renderer.dismiss_for_capture()
            } else {
                renderer.dismiss()
            };
            // Drop both the source and scaled copies of a potentially large
            // grid as soon as the overlay leaves the screen.
            dpi_cache.clear();
            let shutdown = matches!(control, Control::Shutdown(_));
            match control {
                Control::BeginCapture(generation) => {
                    acknowledge_capture(shared, generation, result);
                }
                Control::Dismiss => {
                    if let Err(error) = result {
                        crate::report_error!(
                            "windows-overlay",
                            "Windows overlay dismiss failed: {error}"
                        );
                    }
                }
                Control::Shutdown(reply) => {
                    let _ = reply.send(result);
                }
            }
            if shutdown {
                mark_renderer_stopped(shared, "overlay renderer shut down");
                return;
            }
        }

        if let Some(frame) = frame {
            scale = frame.scale;
            let scene = match dpi_cache.scene_for_dpi(frame.scene.as_ref(), frame.scale) {
                std::borrow::Cow::Borrowed(_) => Arc::clone(&frame.scene),
                std::borrow::Cow::Owned(scene) => Arc::new(scene),
            };
            match renderer.present(scene, frame.area) {
                Ok(Some(notice)) => {
                    crate::app::perf_probe::mark("native_presented");
                    warn(events, notice);
                }
                Ok(None) => crate::app::perf_probe::mark("native_presented"),
                Err(error) => crate::report_error!(
                    "windows-overlay",
                    "Windows overlay render failed; the next frame will retry: {error}"
                ),
            }
        }
        if let Some(mut positions) = positions {
            if scale.is_finite()
                && scale.max(1.0) != 1.0
                && let Some(indicator) = positions.indicator.as_mut()
            {
                indicator.x = indicator.x.round();
                indicator.y = indicator.y.round();
            }
            match renderer.update_positions(positions.cursor, positions.indicator) {
                Ok(Some(notice)) => {
                    crate::app::perf_probe::mark("native_presented");
                    warn(events, notice);
                }
                Ok(None) => crate::app::perf_probe::mark("native_presented"),
                Err(error) => crate::report_error!(
                    "windows-overlay",
                    "Windows overlay position update failed; the next frame will retry: {error}"
                ),
            }
        }
        // Window messages (especially WM_NCHITTEST) must be serviced between
        // frames. Leaving the full-screen HWND on a Condvar makes Windows mark
        // it hung and blocks every click beneath it while normal mode is idle.
        if !native::pump_window_messages() {
            mark_renderer_stopped(shared, "overlay renderer window was destroyed");
            return;
        }
    }
}

enum Renderer {
    Gpu(GpuOverlay),
    Cpu(overlay::Overlay),
}

struct AdaptiveRenderer {
    renderer: Renderer,
    last_gpu_rebuild: Option<Instant>,
    last_scene: Option<Arc<OverlayScene>>,
    last_area: Option<Rect>,
    /// True only when DWM has confirmed that no KeySteer pixels can be part of
    /// the desktop frame. An ordinary dismiss is intentionally asynchronous,
    /// so it cannot make the renderer capture-clean by itself.
    capture_state: CaptureState,
}

#[derive(Debug, Clone, Copy)]
struct CaptureState {
    clean: bool,
}

impl CaptureState {
    const fn new() -> Self {
        Self { clean: true }
    }

    fn presented(&mut self, has_visible_pixels: bool) {
        if has_visible_pixels {
            self.clean = false;
        }
    }

    fn dismissed_asynchronously(&mut self, had_visible_pixels: bool, failed: bool) {
        if had_visible_pixels || failed {
            self.clean = false;
        }
    }

    const fn needs_barrier(self) -> bool {
        !self.clean
    }

    fn confirm_hidden(&mut self) {
        self.clean = true;
    }
}

impl AdaptiveRenderer {
    fn new() -> (Self, Option<String>) {
        match GpuOverlay::new() {
            Ok(gpu) => (
                Self {
                    renderer: Renderer::Gpu(gpu),
                    last_gpu_rebuild: None,
                    last_scene: None,
                    last_area: None,
                    capture_state: CaptureState::new(),
                },
                None,
            ),
            Err(error) => (
                Self {
                    renderer: Renderer::Cpu(overlay::Overlay::new()),
                    last_gpu_rebuild: None,
                    last_scene: None,
                    last_area: None,
                    capture_state: CaptureState::new(),
                },
                Some(format!(
                    "DirectComposition is unavailable; using the DIB renderer: {error}"
                )),
            ),
        }
    }

    fn present(&mut self, scene: Arc<OverlayScene>, area: Rect) -> Result<Option<String>, String> {
        let has_visible_pixels = scene_has_visible_pixels(scene.as_ref());
        let gpu_error = match &mut self.renderer {
            Renderer::Gpu(gpu) => match gpu.present(Arc::clone(&scene), area) {
                Ok(()) => {
                    self.last_scene = Some(scene);
                    self.last_area = Some(area);
                    self.capture_state.presented(has_visible_pixels);
                    return Ok(None);
                }
                Err(error) => error,
            },
            Renderer::Cpu(cpu) => {
                cpu.present(Arc::clone(&scene), area)?;
                self.last_scene = Some(scene);
                self.last_area = Some(area);
                self.capture_state.presented(has_visible_pixels);
                return Ok(None);
            }
        };

        let now = Instant::now();
        if gpu_rebuild_allowed(self.last_gpu_rebuild, now) {
            self.last_gpu_rebuild = Some(now);
            if let Ok(mut replacement) = GpuOverlay::new()
                && replacement.present(Arc::clone(&scene), area).is_ok()
            {
                self.renderer = Renderer::Gpu(replacement);
                self.last_scene = Some(scene);
                self.last_area = Some(area);
                self.capture_state.presented(has_visible_pixels);
                return Ok(Some(format!(
                    "DirectComposition device was rebuilt after a render failure: {gpu_error}"
                )));
            }
        }

        let mut cpu = overlay::Overlay::new();
        cpu.present(Arc::clone(&scene), area)?;
        self.renderer = Renderer::Cpu(cpu);
        self.last_scene = Some(scene);
        self.last_area = Some(area);
        self.capture_state.presented(has_visible_pixels);
        Ok(Some(format!(
            "DirectComposition failed repeatedly; using the DIB renderer for this session: {gpu_error}"
        )))
    }

    fn update_positions(
        &mut self,
        cursor: Option<Point>,
        indicator: Option<Point>,
    ) -> Result<Option<String>, String> {
        if let Renderer::Gpu(gpu) = &mut self.renderer
            && gpu.update_positions(cursor, indicator).is_ok()
        {
            return Ok(None);
        }

        let area = self
            .last_area
            .ok_or("overlay position update arrived before the first complete frame")?;
        let mut scene = self
            .last_scene
            .as_deref()
            .cloned()
            .ok_or("overlay position update arrived before the first complete frame")?;
        if let (Some(position), Some(marker)) = (cursor, scene.cursor_marker.as_mut()) {
            marker.center = position;
        }
        if let (Some(position), Some(item)) = (indicator, scene.indicator.as_mut()) {
            item.position = position;
        }
        self.present(Arc::new(scene), area)
    }

    fn dismiss(&mut self) -> Result<(), String> {
        let had_visible_scene = self
            .last_scene
            .as_deref()
            .is_some_and(scene_has_visible_pixels);
        let result = match &mut self.renderer {
            Renderer::Gpu(gpu) => gpu.dismiss(),
            Renderer::Cpu(cpu) => cpu.dismiss(),
        };
        self.last_scene = None;
        self.last_area = None;
        self.capture_state
            .dismissed_asynchronously(had_visible_scene, result.is_err());
        result
    }

    fn dismiss_for_capture(&mut self) -> Result<(), String> {
        // GPU dismiss only queues the tree removal. DwmFlush is the one
        // compositor barrier for capture and confirms that the removed tree is
        // no longer part of the desktop frame. CPU dismiss destroys its HWND
        // synchronously, then uses the same barrier for a common ACK contract.
        self.dismiss()?;
        if !self.capture_state.needs_barrier() {
            crate::app::perf_probe::mark("capture_barrier_skipped");
            return Ok(());
        }
        crate::app::perf_probe::mark("capture_barrier_started");
        native::wait_for_dwm_frame()
            .map_err(|error| format!("DWM did not confirm the hidden overlay frame: {error}"))?;
        self.capture_state.confirm_hidden();
        Ok(())
    }
}

fn scene_has_visible_pixels(scene: &OverlayScene) -> bool {
    !scene.is_empty() || scene.backdrop.is_some()
}

fn acknowledge_capture(shared: &Shared, generation: u64, result: Result<(), String>) {
    let reply = {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let reply = state
            .capture
            .as_mut()
            .filter(|gate| gate.generation == generation)
            .and_then(|gate| gate.ready.take());
        if reply.is_some() && result.is_ok() {
            state.phase = OverlayPhase::HiddenForCapture(generation);
        }
        reply
    };
    if let Some(reply) = reply {
        let _ = reply.send(result);
    }
}

fn cancel_capture(shared: &Shared, generation: u64, reason: &str) {
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if state
        .capture
        .as_ref()
        .is_some_and(|gate| gate.generation == generation)
    {
        cancel_capture_locked(&mut state, reason);
    }
}

fn cancel_capture_locked(state: &mut State, reason: &str) {
    if let Some(mut gate) = state.capture.take()
        && let Some(reply) = gate.ready.take()
    {
        let _ = reply.send(Err(reason.into()));
    }
    if state.phase != OverlayPhase::Stopping {
        state.phase = OverlayPhase::Normal;
    }
}

fn ensure_renderer_alive(state: &State) -> Result<(), String> {
    if state.alive {
        Ok(())
    } else {
        Err("Windows overlay renderer has already stopped".into())
    }
}

fn mark_renderer_stopped(shared: &Shared, reason: &str) {
    fail_renderer(shared, reason);
}

fn fail_renderer(shared: &Shared, reason: &str) {
    let (capture_reply, control_reply) = {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.alive = false;
        state.wake_pending = false;
        state.phase = OverlayPhase::Stopping;
        state.latest = None;
        state.positions = None;
        let capture_reply = state.capture.take().and_then(|mut gate| gate.ready.take());
        let control_reply = state.control.take().and_then(|control| match control {
            Control::Dismiss | Control::BeginCapture(_) => None,
            Control::Shutdown(reply) => Some(reply),
        });
        (capture_reply, control_reply)
    };
    if let Some(reply) = capture_reply {
        let _ = reply.send(Err(reason.into()));
    }
    if let Some(reply) = control_reply {
        let _ = reply.send(Err(reason.into()));
    }
}

fn warn(events: &EventSender, message: String) {
    let _ = events.send(crate::api::backend::BackendEvent::Warning(message));
}

fn gpu_rebuild_allowed(last_rebuild: Option<Instant>, now: Instant) -> bool {
    last_rebuild.is_none_or(|previous| now.duration_since(previous) >= DEVICE_FAILURE_WINDOW)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::overlay::Color;
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::HTTRANSPARENT;

    #[test]
    fn second_device_failure_inside_window_disables_rebuild() {
        let now = Instant::now();
        assert!(gpu_rebuild_allowed(None, now));
        assert!(!gpu_rebuild_allowed(
            Some(now - Duration::from_secs(59)),
            now
        ));
        assert!(gpu_rebuild_allowed(
            Some(now - Duration::from_secs(60)),
            now
        ));
    }

    #[test]
    fn capture_state_skips_only_confirmed_clean_frames() {
        for _ in 0..1_000 {
            let mut state = CaptureState::new();
            assert!(!state.needs_barrier());

            state.presented(false);
            assert!(!state.needs_barrier());

            state.presented(true);
            assert!(state.needs_barrier());
            state.dismissed_asynchronously(true, false);
            assert!(state.needs_barrier());

            state.confirm_hidden();
            assert!(!state.needs_barrier());
            state.dismissed_asynchronously(false, false);
            assert!(!state.needs_barrier());
        }
    }

    #[test]
    fn stale_capture_lease_never_releases_new_generation() {
        for generation in 1..=1_000 {
            let shared = Arc::new(Shared::default());
            {
                let mut state = shared.state.lock().expect("capture state");
                state.alive = false;
                state.capture = Some(CaptureGate {
                    generation: generation + 1,
                    ready: None,
                    deferred_frame: None,
                    deferred_positions: None,
                });
                state.phase = OverlayPhase::HidingForCapture(generation + 1);
            }
            let mut stale = CaptureLease {
                shared: Arc::clone(&shared),
                thread_id: 0,
                generation,
                ready: None,
                released: false,
            };
            stale.release_inner(true).expect("stale release");
            assert_eq!(
                shared
                    .state
                    .lock()
                    .expect("capture state")
                    .capture
                    .as_ref()
                    .map(|gate| gate.generation),
                Some(generation + 1)
            );
            assert_eq!(
                shared.state.lock().expect("capture state").phase,
                OverlayPhase::HidingForCapture(generation + 1)
            );
        }
    }

    #[test]
    fn capture_gate_keeps_only_latest_deferred_frame() {
        let shared = Arc::new(Shared::default());
        let area = Rect::new(0.0, 0.0, 32.0, 32.0);
        {
            let mut state = shared.state.lock().expect("capture state");
            state.wake_pending = true;
            state.capture = Some(CaptureGate {
                generation: 7,
                ready: None,
                deferred_frame: Some(Frame {
                    scene: Arc::new(OverlayScene::new()),
                    area,
                    scale: 2.0,
                }),
                deferred_positions: Some(Positions {
                    cursor: Some(Point::new(9.0, 10.0)),
                    indicator: None,
                }),
            });
            state.phase = OverlayPhase::HiddenForCapture(7);
        }
        let mut lease = CaptureLease {
            shared: Arc::clone(&shared),
            thread_id: 0,
            generation: 7,
            ready: None,
            released: false,
        };
        lease.release_inner(true).expect("capture release");
        let state = shared.state.lock().expect("capture state");
        assert_eq!(state.latest.as_ref().map(|frame| frame.scale), Some(2.0));
        assert_eq!(
            state.positions.and_then(|positions| positions.cursor),
            Some(Point::new(9.0, 10.0))
        );
        assert_eq!(state.phase, OverlayPhase::Normal);
    }

    #[test]
    fn capture_gate_drops_positions_until_a_complete_frame_exists() {
        let shared = Arc::new(Shared::default());
        {
            let mut state = shared.state.lock().expect("capture state");
            state.capture = Some(CaptureGate {
                generation: 11,
                ready: None,
                deferred_frame: None,
                deferred_positions: None,
            });
            state.phase = OverlayPhase::HiddenForCapture(11);
        }
        let worker = OverlayWorker {
            shared: Arc::clone(&shared),
            thread_id: 0,
            worker: None,
        };
        for index in 0..8_000 {
            worker
                .update_positions(Some(Point::new(index as f64, 1.0)), None)
                .expect("capture position update");
        }
        {
            let state = shared.state.lock().expect("capture state");
            assert!(
                state
                    .capture
                    .as_ref()
                    .is_some_and(|gate| gate.deferred_positions.is_none())
            );
            assert!(!state.wake_pending);
        }
        let mut lease = CaptureLease {
            shared: Arc::clone(&shared),
            thread_id: 0,
            generation: 11,
            ready: None,
            released: false,
        };
        lease.release_inner(true).expect("capture release");
        let state = shared.state.lock().expect("capture state");
        assert!(state.latest.is_none());
        assert!(state.positions.is_none());
        assert!(!state.wake_pending);
        assert_eq!(state.phase, OverlayPhase::Normal);
    }

    #[test]
    fn stopped_renderer_drops_pending_state_and_notifies_waiters() {
        let shared = Shared::default();
        let (capture_tx, capture_rx) = mpsc::sync_channel(1);
        let (control_tx, control_rx) = mpsc::sync_channel(1);
        {
            let mut state = shared.state.lock().expect("renderer state");
            state.latest = Some(Frame {
                scene: Arc::new(OverlayScene::new()),
                area: Rect::new(0.0, 0.0, 32.0, 32.0),
                scale: 1.0,
            });
            state.positions = Some(Positions {
                cursor: Some(Point::new(4.0, 5.0)),
                indicator: None,
            });
            state.capture = Some(CaptureGate {
                generation: 13,
                ready: Some(capture_tx),
                deferred_frame: None,
                deferred_positions: None,
            });
            state.control = Some(Control::Shutdown(control_tx));
            state.wake_pending = true;
        }
        fail_renderer(&shared, "renderer failed");
        assert_eq!(
            capture_rx.recv().expect("capture reply"),
            Err("renderer failed".into())
        );
        assert_eq!(
            control_rx.recv().expect("control reply"),
            Err("renderer failed".into())
        );
        let state = shared.state.lock().expect("renderer state");
        assert!(!state.alive);
        assert!(!state.wake_pending);
        assert!(state.latest.is_none());
        assert!(state.positions.is_none());
        assert!(state.capture.is_none());
        assert!(state.control.is_none());
        assert_eq!(state.phase, OverlayPhase::Stopping);
    }

    #[test]
    fn position_burst_requires_one_wake_and_keeps_only_the_last_value() {
        let mut state = State::default();
        let mut wakes = 0;
        for index in 0..8_000 {
            state.positions = Some(Positions {
                cursor: Some(Point::new(index as f64, 1.0)),
                indicator: None,
            });
            wakes += usize::from(mark_wake_pending(&mut state).expect("live renderer"));
        }
        assert_eq!(wakes, 1);
        assert_eq!(
            state.positions.and_then(|positions| positions.cursor),
            Some(Point::new(7_999.0, 1.0))
        );
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn idle_overlay_worker_remains_responsive_and_click_through() -> Result<(), String> {
        let (event_tx, _event_rx) = mpsc::channel();
        let events = EventSender::without_wake(event_tx);
        let mut worker = OverlayWorker::start(events)?;
        let area = Rect::new(0.0, 0.0, 64.0, 64.0);
        let mut scene = OverlayScene::new();
        scene.clip = Some(area);
        scene.backdrop = Some(Color::rgba(0, 0, 0, 1));
        worker.present(Arc::new(scene), area, 1.0)?;

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match native::probe_overlay_hit_test(
                &[
                    super::super::gpu_overlay::GpuOverlay::CLASS_NAME,
                    super::super::overlay::Overlay::CLASS_NAME,
                ],
                250,
            ) {
                Ok(Some(probe)) => {
                    assert_eq!(probe.hit_test, LRESULT(HTTRANSPARENT as isize));
                    let required = (windows::Win32::UI::WindowsAndMessaging::WS_EX_LAYERED
                        | windows::Win32::UI::WindowsAndMessaging::WS_EX_TRANSPARENT)
                        .0;
                    assert_eq!(probe.ex_style & required, required);
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => return Err("overlay HWND was not created".into()),
                Err(error) => return Err(format!("overlay HWND stopped responding: {error}")),
            }
        }

        worker.dismiss()?;
        worker.stop()
    }
}
