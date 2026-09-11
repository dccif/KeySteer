#![cfg(test)]

use super::input_router::CompiledKeymap;
use super::*;
use crate::TEST_ALLOCATOR;
use crate::api::CommandBatch;
use crate::api::Mode;
use crate::api::binding::Direction;
use crate::api::geometry::Rect;
use crate::api::input::InputEvent;
use crate::config::{Config, ConfigStore};
use stats_alloc::Region;
use std::sync::{Arc, Mutex};

fn attach_test_repository(
    engine: &mut Engine,
    config: Config,
    store: Option<ConfigStore>,
    discovery_directory: Option<std::path::PathBuf>,
    source: String,
) {
    engine.attach_configuration(Box::new(crate::app::configuration::ConfigRepository::new(
        config,
        source,
        store,
        discovery_directory,
    )));
}

fn active_config(engine: &Engine) -> Config {
    Config::parse(
        &engine
            .configuration
            .as_ref()
            .expect("test engine has a configuration repository")
            .source_text()
            .unwrap(),
    )
    .unwrap()
}

/// Records what the engine asked of the platform.
#[derive(Default)]
struct Recorder {
    text_prompts: Vec<crate::api::window_presets::TextPrompt>,
    cancelled_text_prompts: Vec<u64>,
    window_requests: Vec<crate::api::window::WindowRequest>,
    cancelled_window_sessions: Vec<u64>,
    character_bindings: Vec<Vec<String>>,
    window_moves: Vec<crate::api::command::WindowScreenTarget>,
    presents: usize,
    dismissals: usize,
    warps: Vec<Point>,
    moves: Vec<(f64, f64)>,
    scrolls: Vec<(f64, f64)>,
    scenes: Vec<OverlayScene>,
    positions: Vec<(Option<Point>, Option<Point>)>,
    frame_clock_states: Vec<bool>,
    timeline: Vec<&'static str>,
    dispositions: Vec<KeyDisposition>,
    scans: usize,
    scan_requests: Vec<crate::api::UiScanRequest>,
    cancelled_scans: Vec<u64>,
    opened_urls: Vec<String>,
    shutdowns: usize,
    /// Button press/release/click calls, in order.
    buttons: Vec<(MouseButton, ButtonAction)>,
    /// Number of `Click` actions, for convenience.
    clicks: usize,
    /// Synthetic keystrokes, in order.
    sent: Vec<(String, KeyState)>,
    fail_next_key_up: bool,
    fail_next_mouse_release: bool,
    position_update_attempts: usize,
}

struct FakeBackend {
    fail_next_disposition: bool,
    window_move_pointer: Option<Point>,
    fail_window_move: bool,
    events: Vec<BackendEvent>,
    log: Arc<Mutex<Recorder>>,
    fail_start: bool,
    fail_warp: bool,
    fail_mouse: bool,
    accept_position_updates: bool,
    fail_position_updates: bool,
}

impl FakeBackend {
    fn new(events: Vec<BackendEvent>) -> (Self, Arc<Mutex<Recorder>>) {
        let log = Arc::new(Mutex::new(Recorder::default()));
        (
            Self {
                fail_next_disposition: false,
                window_move_pointer: None,
                fail_window_move: false,
                events,
                log: log.clone(),
                fail_start: false,
                fail_warp: false,
                fail_mouse: false,
                accept_position_updates: false,
                fail_position_updates: false,
            },
            log,
        )
    }
}

impl Backend for FakeBackend {
    fn request_text_prompt(
        &mut self,
        prompt: crate::api::window_presets::TextPrompt,
    ) -> Result<(), String> {
        self.log.lock().unwrap().text_prompts.push(prompt);
        Ok(())
    }
    fn cancel_text_prompt(&mut self, id: u64) {
        self.log.lock().unwrap().cancelled_text_prompts.push(id);
    }
    fn request_window(&mut self, request: crate::api::window::WindowRequest) -> Result<(), String> {
        let mut log = self.log.lock().unwrap();
        log.timeline.push("window");
        log.window_requests.push(request);
        Ok(())
    }
    fn cancel_window_session(&mut self, session: u64) {
        self.log
            .lock()
            .unwrap()
            .cancelled_window_sessions
            .push(session);
    }
    fn set_character_bindings(&mut self, keys: &[Key]) {
        let mut keys: Vec<_> = keys.iter().map(|key| key.as_str().to_string()).collect();
        keys.sort();
        self.log.lock().unwrap().character_bindings.push(keys);
    }

    fn start(&mut self) -> Result<(), String> {
        if self.fail_start {
            Err("injected startup failure".into())
        } else {
            Ok(())
        }
    }

    fn poll(&mut self, _t: Duration) -> Result<Option<BackendEvent>, String> {
        // Quit once the script is exhausted so `run` terminates.
        Ok(Some(if self.events.is_empty() {
            BackendEvent::Quit
        } else {
            self.events.remove(0)
        }))
    }
    fn dispose_key(&mut self, d: KeyDisposition) -> Result<(), String> {
        if std::mem::take(&mut self.fail_next_disposition) {
            return Err("keyboard disposition deadline expired".into());
        }
        let mut log = self.log.lock().unwrap();
        log.dispositions.push(d);
        log.timeline.push("dispose");
        Ok(())
    }
    fn screens(&self) -> Result<Vec<Screen>, String> {
        Ok(vec![Screen {
            bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
            work_area: Rect::new(0.0, 0.0, 1000.0, 800.0),
            is_primary: true,
            scale: 1.0,
            name: None,
        }])
    }
    fn pointer(&self) -> Result<Point, String> {
        Ok(Point::new(10.0, 10.0))
    }
    fn move_window_to_screen(
        &self,
        target: crate::api::command::WindowScreenTarget,
    ) -> Result<Option<Point>, String> {
        self.log.lock().unwrap().window_moves.push(target);
        if self.fail_window_move {
            return Err("injected window movement failure".into());
        }
        Ok(self.window_move_pointer)
    }
    fn focused_app(&self) -> Result<Option<FocusedApp>, String> {
        Ok(None)
    }
    fn warp_pointer(&self, to: Point) -> Result<(), String> {
        if self.fail_warp {
            return Err("injected warp failure".into());
        }
        self.log.lock().unwrap().warps.push(to);
        Ok(())
    }
    fn move_pointer(&self, _from: Point, dx: f64, dy: f64) -> Result<(), String> {
        let mut log = self.log.lock().unwrap();
        log.moves.push((dx, dy));
        log.timeline.push("move");
        Ok(())
    }
    fn mouse_button(&self, b: MouseButton, a: ButtonAction) -> Result<(), String> {
        if self.fail_mouse {
            return Err("SendInput blocked by UIPI".into());
        }
        let mut log = self.log.lock().unwrap();
        if a == ButtonAction::Release && log.fail_next_mouse_release {
            log.fail_next_mouse_release = false;
            return Err("injected mouse release failure".into());
        }
        log.buttons.push((b, a));
        log.timeline.push("mouse");
        if a == ButtonAction::Click {
            log.clicks += 1;
        }
        Ok(())
    }
    fn scroll(&self, dx: f64, dy: f64) -> Result<(), String> {
        self.log.lock().unwrap().scrolls.push((dx, dy));
        Ok(())
    }
    fn send_key(&self, k: &Key, s: KeyState) -> Result<(), String> {
        let mut log = self.log.lock().unwrap();
        if s == KeyState::Up && log.fail_next_key_up {
            log.fail_next_key_up = false;
            return Err("injected key-up failure".into());
        }
        log.sent.push((k.as_str().to_string(), s));
        Ok(())
    }
    fn set_frame_clock(&mut self, active: bool) -> Result<(), String> {
        self.log.lock().unwrap().frame_clock_states.push(active);
        Ok(())
    }
    fn present(&mut self, scene: Arc<OverlayScene>) -> Result<(), String> {
        let mut log = self.log.lock().unwrap();
        log.presents += 1;
        log.scenes.push(scene.as_ref().clone());
        log.timeline.push("present");
        Ok(())
    }
    fn update_overlay_positions(
        &mut self,
        cursor: Option<Point>,
        indicator: Option<Point>,
    ) -> Result<bool, String> {
        self.log.lock().unwrap().position_update_attempts += 1;
        if self.fail_position_updates {
            return Err("injected position update failure".into());
        }
        self.log.lock().unwrap().positions.push((cursor, indicator));
        Ok(self.accept_position_updates)
    }
    fn dismiss(&mut self) -> Result<(), String> {
        self.log.lock().unwrap().dismissals += 1;
        Ok(())
    }
    fn request_ui_scan(&mut self, request: crate::api::UiScanRequest) -> Result<(), String> {
        self.log.lock().unwrap().scans += 1;
        self.log.lock().unwrap().scan_requests.push(request);
        Ok(())
    }
    fn cancel_ui_scan(&mut self, id: u64) -> Result<(), String> {
        self.log.lock().unwrap().cancelled_scans.push(id);
        Ok(())
    }
    fn open_url(&mut self, url: &str) -> Result<(), String> {
        self.log.lock().unwrap().opened_urls.push(url.to_string());
        Ok(())
    }
    fn shutdown(&mut self) -> Result<(), String> {
        self.log.lock().unwrap().shutdowns += 1;
        Ok(())
    }
    fn name(&self) -> &'static str {
        "fake"
    }
}

/// Minimal mode that records the events it saw and emits scripted commands.
struct ProbeMode {
    id: ModeId,
    captures: bool,
    seen: Arc<Mutex<Vec<String>>>,
    on_key: Vec<Command>,
}

impl ProbeMode {
    fn new(id: &str, seen: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            id: ModeId::new(id).unwrap(),
            captures: true,
            seen,
            on_key: Vec::new(),
        }
    }
}

impl Mode for ProbeMode {
    fn id(&self) -> ModeId {
        self.id.clone()
    }
    fn captures_keyboard(&self) -> bool {
        self.captures
    }
    fn claims_key(&self, key: &Key) -> bool {
        match self.id.as_str() {
            "grid" => key.as_char() == Some('g'),
            "recursive_grid" => key.as_char() == Some('r'),
            "ui_hint" => key
                .as_char()
                .is_some_and(|character| character.is_ascii_alphanumeric()),
            _ => false,
        }
    }
    fn handle(&mut self, event: &ModeEvent, _ctx: &HostContext<'_>) -> CommandBatch {
        let label = match event {
            ModeEvent::Activated { .. } => "activated",
            ModeEvent::Pushed { .. } => "pushed",
            ModeEvent::Deactivated => "deactivated",
            ModeEvent::Suspended => "suspended",
            ModeEvent::Resumed => "resumed",
            ModeEvent::Restarted => "restarted",
            ModeEvent::FinishRequested { .. } => "finish_requested",
            ModeEvent::Clicked { .. } => "clicked",
            ModeEvent::Key { .. } => "key",
            ModeEvent::Binding { binding, .. } => &format!("binding({binding})"),
            ModeEvent::Invoked { .. } => "invoked",
            ModeEvent::Timer { .. } => "timer",
            ModeEvent::ScreensChanged(_) => "screens",
            ModeEvent::ScreenRetargeted { .. } => "retargeted",
            ModeEvent::UiScanned(_) => "scanned",
            ModeEvent::PointerMoved(_) => "pointer",
            ModeEvent::Frame { .. } => "frame",
            ModeEvent::FocusChanged(_) => "focus",
            ModeEvent::WindowPresets(_) => "window-presets",
            ModeEvent::WindowResult(_) => "window",
            ModeEvent::TemporaryModeChanged { .. } => "temporary",
        };
        self.seen
            .lock()
            .unwrap()
            .push(format!("{}:{label}", self.id));
        CommandBatch::from(match event {
            ModeEvent::Key { .. } => self.on_key.clone(),
            _ => Vec::new(),
        })
    }
}

struct OwnedRouteProbe {
    calls: Arc<Mutex<(usize, usize)>>,
}

impl Mode for OwnedRouteProbe {
    fn id(&self) -> ModeId {
        ModeId::idle()
    }

    fn handle(&mut self, _event: &ModeEvent, _ctx: &HostContext<'_>) -> CommandBatch {
        self.calls.lock().unwrap().0 += 1;
        CommandBatch::new()
    }

    fn handle_owned(&mut self, event: ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        self.calls.lock().unwrap().1 += 1;
        self.handle(&event, ctx)
    }
}

#[test]
fn only_ui_scan_results_use_the_owned_mode_route() {
    let calls = Arc::new(Mutex::new((0, 0)));
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.register(Box::new(OwnedRouteProbe {
        calls: Arc::clone(&calls),
    }));
    let (mut backend, _) = FakeBackend::new(Vec::new());

    engine
        .dispatch(
            ModeEvent::Frame {
                elapsed: Duration::from_millis(8),
            },
            &mut backend,
        )
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), (1, 0));

    engine.scan_owners.insert(91, ModeId::idle());
    engine
        .handle_backend_event(
            BackendEvent::UiScanned(crate::api::UiScanResult {
                id: 91,
                targets: Vec::new(),
                status: UiScanStatus::Success,
            }),
            &mut backend,
        )
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), (2, 1));
}

fn key_event(name: &str, state: KeyState) -> BackendEvent {
    BackendEvent::Input(InputEvent {
        character: None,
        key: Key::new(name).unwrap(),
        state,
        repeat: false,
        injected: false,
        timestamp_millis: 0,
    })
}

fn key_down(name: &str) -> BackendEvent {
    key_event(name, KeyState::Down)
}

fn key_up(name: &str) -> BackendEvent {
    key_event(name, KeyState::Up)
}

/// An engine whose `normal` table contains exactly `chord -> binding`,
/// with probes registered for idle and normal.
fn engine_with_normal_binding(chord: &str, binding: &str) -> Engine {
    engine_with_normal_action(chord, Binding::parse(binding).unwrap())
}

fn engine_with_normal_action(chord: &str, binding: Binding) -> Engine {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config.normal.bindings.insert(chord.into(), binding);
    // Keep a way back out so the mode is escapable.
    config.normal.bindings.insert("esc".into(), Binding::Escape);

    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));
    engine
}

/// Like [`engine_with_normal_binding`], but the caller keeps the probe log
/// so it can assert on what the mode saw.
fn engine_with_normal_probes(seen: &Arc<Mutex<Vec<String>>>, chord: &str, binding: &str) -> Engine {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config
        .normal
        .bindings
        .insert(chord.into(), Binding::parse(binding).unwrap());

    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen.clone())));
    engine
}

/// Press then release every key of `chord`, in a realistic order.
///
/// Modifiers go down first and come up last, which is what a keyboard
/// actually produces.
fn tap_chord(chord: &str) -> Vec<BackendEvent> {
    let parsed = KeyChord::parse(chord).unwrap();
    let (modifiers, keys): (Vec<&Key>, Vec<&Key>) =
        parsed.keys().iter().partition(|k| k.is_modifier());

    let mut events = Vec::new();
    for key in &modifiers {
        events.push(key_event(key.as_str(), KeyState::Down));
    }
    for key in &keys {
        events.push(key_event(key.as_str(), KeyState::Down));
    }
    for key in keys.iter().rev() {
        events.push(key_event(key.as_str(), KeyState::Up));
    }
    for key in modifiers.iter().rev() {
        events.push(key_event(key.as_str(), KeyState::Up));
    }
    events
}

/// The chord that enters `normal` in the default configuration.
///
/// Read from the config rather than hard-coded, so changing the default
/// cannot silently invalidate every test below.
fn normal_launcher() -> String {
    Config::default()
        .hotkeys
        .iter()
        .find(|(_, b)| b.mode() == Some(&ModeId::normal()))
        .map(|(chord, _)| chord.clone())
        .expect("a default binding must enter normal")
}

/// The key sequence that enters `normal` from `idle`.
fn enter_normal() -> Vec<BackendEvent> {
    tap_chord(&normal_launcher())
}

/// Run `events` after entering `normal`.
fn run_in_normal(engine: &mut Engine, events: Vec<BackendEvent>) -> Arc<Mutex<Recorder>> {
    let mut script = enter_normal();
    script.extend(events);
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();
    log
}

/// Register probes for idle, normal and one extra mode.
fn engine_with_probes(seen: &Arc<Mutex<Vec<String>>>, extra: &[&str]) -> Engine {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen.clone())));
    for id in extra {
        engine.register(Box::new(ProbeMode::new(id, seen.clone())));
    }
    engine
}

include!("performance.rs");
include!("input.rs");
include!("drag.rs");
include!("modes.rs");
include!("toggle.rs");
include!("lifecycle.rs");
include!("overlay.rs");
include!("reload_scheduler.rs");
include!("window_mover.rs");
include!("window_mode.rs");
