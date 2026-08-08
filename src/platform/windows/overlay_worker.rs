//! Non-blocking latest-frame submission for the Windows overlay.
//!
//! The engine/input thread only replaces one pending frame. A dedicated
//! normal-priority thread owns every HWND and GDI resource, so a dense scene
//! cannot delay the synchronous hook disposition or native click injection.

use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::api::geometry::Rect;
use crate::api::overlay::OverlayScene;

use super::{EventSender, gpu_overlay::GpuOverlay, native, overlay};

const DEVICE_FAILURE_WINDOW: Duration = Duration::from_secs(60);
const RENDER_WAKE_MESSAGE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x50;

struct Frame {
    scene: Arc<OverlayScene>,
    area: Rect,
    scale: f64,
}

enum Control {
    Dismiss(SyncSender<Result<(), String>>),
    Shutdown(SyncSender<Result<(), String>>),
}

#[derive(Default)]
struct State {
    latest: Option<Frame>,
    control: Option<Control>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
}

pub(super) struct OverlayWorker {
    shared: Arc<Shared>,
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl OverlayWorker {
    pub(super) fn start(events: EventSender) -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let join = std::thread::Builder::new()
            .name("keysteer-overlay-render".into())
            .spawn(move || render_loop(&thread_shared, &events, ready_tx))
            .map_err(|error| format!("cannot start Windows overlay renderer: {error}"))?;
        let thread_id = match ready_rx.recv() {
            Ok(thread_id) => thread_id,
            Err(_) => {
                let _ = join.join();
                return Err("Windows overlay renderer stopped before becoming ready".into());
            }
        };
        Ok(Self {
            shared,
            thread_id,
            join: Some(join),
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
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.latest = Some(Frame { scene, area, scale });
        }
        self.wake_renderer()
    }

    pub(super) fn dismiss(&self) -> Result<(), String> {
        self.control(Control::Dismiss)
    }

    fn control(
        &self,
        make: impl FnOnce(SyncSender<Result<(), String>>) -> Control,
    ) -> Result<(), String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.latest = None;
            state.control = Some(make(reply_tx));
        }
        self.wake_renderer()?;
        reply_rx
            .recv()
            .unwrap_or_else(|_| Err("Windows overlay renderer stopped unexpectedly".into()))
    }

    fn wake_renderer(&self) -> Result<(), String> {
        native::post_thread_wake(self.thread_id, RENDER_WAKE_MESSAGE)
            .map_err(|error| format!("cannot wake Windows overlay renderer: {error}"))
    }

    fn stop(&mut self) -> Result<(), String> {
        let result = if self.join.is_some() {
            self.control(Control::Shutdown)
        } else {
            Ok(())
        };
        if let Some(join) = self.join.take()
            && join.join().is_err()
        {
            return Err("Windows overlay renderer panicked during shutdown".into());
        }
        result
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
        return;
    }
    let (mut renderer, startup_notice) = AdaptiveRenderer::new();
    let mut dpi_cache = overlay::DpiSceneCache::default();
    if let Some(notice) = startup_notice {
        warn(events, notice);
    }
    loop {
        let (control, frame) = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (state.control.take(), state.latest.take())
        };

        if control.is_none() && frame.is_none() {
            match native::wait_and_dispatch_window_message() {
                Ok(true) => continue,
                Ok(false) => return,
                Err(error) => {
                    warn(
                        events,
                        format!("Windows overlay message loop failed: {error}"),
                    );
                    return;
                }
            }
        }

        if let Some(control) = control {
            let result = renderer.dismiss();
            // Drop both the source and scaled copies of a potentially large
            // grid as soon as the overlay leaves the screen.
            dpi_cache.clear();
            let shutdown = matches!(control, Control::Shutdown(_));
            match control {
                Control::Dismiss(reply) | Control::Shutdown(reply) => {
                    let _ = reply.send(result);
                }
            }
            if shutdown {
                return;
            }
        }

        if let Some(frame) = frame {
            let scene = match dpi_cache.scene_for_dpi(frame.scene.as_ref(), frame.scale) {
                std::borrow::Cow::Borrowed(_) => Arc::clone(&frame.scene),
                std::borrow::Cow::Owned(scene) => Arc::new(scene),
            };
            match renderer.present(scene, frame.area) {
                Ok(Some(notice)) => warn(events, notice),
                Ok(None) => {}
                Err(error) => warn(
                    events,
                    format!("Windows overlay render failed; the next frame will retry: {error}"),
                ),
            }
        }
        // Window messages (especially WM_NCHITTEST) must be serviced between
        // frames. Leaving the full-screen HWND on a Condvar makes Windows mark
        // it hung and blocks every click beneath it while normal mode is idle.
        if !native::pump_window_messages() {
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
}

impl AdaptiveRenderer {
    fn new() -> (Self, Option<String>) {
        match GpuOverlay::new() {
            Ok(gpu) => (
                Self {
                    renderer: Renderer::Gpu(gpu),
                    last_gpu_rebuild: None,
                },
                None,
            ),
            Err(error) => (
                Self {
                    renderer: Renderer::Cpu(overlay::Overlay::new()),
                    last_gpu_rebuild: None,
                },
                Some(format!(
                    "DirectComposition is unavailable; using the DIB renderer: {error}"
                )),
            ),
        }
    }

    fn present(&mut self, scene: Arc<OverlayScene>, area: Rect) -> Result<Option<String>, String> {
        let gpu_error = match &mut self.renderer {
            Renderer::Gpu(gpu) => match gpu.present(Arc::clone(&scene), area) {
                Ok(()) => return Ok(None),
                Err(error) => error,
            },
            Renderer::Cpu(cpu) => {
                cpu.present(scene, area)?;
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
                return Ok(Some(format!(
                    "DirectComposition device was rebuilt after a render failure: {gpu_error}"
                )));
            }
        }

        let mut cpu = overlay::Overlay::new();
        cpu.present(scene, area)?;
        self.renderer = Renderer::Cpu(cpu);
        Ok(Some(format!(
            "DirectComposition failed repeatedly; using the DIB renderer for this session: {gpu_error}"
        )))
    }

    fn dismiss(&mut self) -> Result<(), String> {
        match &mut self.renderer {
            Renderer::Gpu(gpu) => gpu.dismiss(),
            Renderer::Cpu(cpu) => cpu.dismiss(),
        }
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
