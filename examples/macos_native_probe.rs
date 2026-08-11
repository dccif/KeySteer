#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos_native_probe must run on macOS 14 or later");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    if let Err(error) = macos::run() {
        eprintln!("macos_native_probe failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use keysteer::api::command::{
        FocusedApp, UiScanRequest, UiScanStatus, UiScanStrategy, VisionOptions,
    };
    use keysteer::api::input::{Key, KeyState};
    use keysteer::api::overlay::{
        Color, CursorMarker, Indicator, LabelStyle, OverlayLabel, OverlayScene, OverlayShape,
    };
    use keysteer::api::{Backend, BackendEvent, KeyDisposition, Point, Rect, Screen, UiTarget};
    use keysteer::platform::macos::MacOsBackend;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton, NSPanel,
        NSView, NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    const MOTION_SAMPLES: usize = 500;
    const INPUT_SAMPLES: usize = 100;
    const SCAN_SAMPLES: usize = 5;

    pub(super) fn run() -> Result<(), String> {
        if std::env::args().any(|argument| argument == "--fixture") {
            return run_fixture();
        }

        let backend_started = Instant::now();
        let mut backend = MacOsBackend::new()?;
        println!(
            "native_backend_ready elapsed_ns={}",
            backend_started.elapsed().as_nanos()
        );
        process_metrics("backend_ready");

        let mut fixture = FixtureProcess::spawn()?;
        std::thread::sleep(Duration::from_millis(250));
        let _ = backend.poll(Duration::from_millis(25));

        probe_overlay(&mut backend)?;
        probe_input(&backend)?;
        probe_scans(&mut backend, fixture.pid())?;
        process_metrics("scan_steady");

        fixture.stop();
        backend.shutdown()?;
        Ok(())
    }

    fn probe_overlay(backend: &mut MacOsBackend) -> Result<(), String> {
        let screens = backend.screens()?;
        let area = Screen::primary(&screens)
            .or_else(|| screens.first())
            .map(|screen| screen.bounds)
            .ok_or_else(|| "macOS native probe requires a display".to_string())?;
        let clip = Rect::new(
            area.x,
            area.y,
            area.width.min(1280.0),
            area.height.min(900.0),
        );
        let mut scene = OverlayScene::with_capacity(24, 24);
        scene.clip = Some(clip);
        for index in 0..24 {
            let x = clip.x + 20.0 + f64::from(index % 6) * 150.0;
            let y = clip.y + 20.0 + f64::from(index / 6) * 90.0;
            let rect = Rect::new(x, y, 120.0, 56.0);
            scene.shapes.push(OverlayShape::outline(
                rect,
                Color::rgba(80, 120, 240, 180),
                1.0,
            ));
            scene.labels.push(OverlayLabel::new(
                format!("Probe {index:02}"),
                rect,
                LabelStyle::default(),
            ));
        }
        scene.cursor_marker = Some(CursorMarker {
            center: Point::new(clip.x + 120.0, clip.y + 120.0),
            radius: 13.0,
            fill: Color::rgba(90, 130, 255, 48),
            stroke: Color::rgba(120, 160, 255, 230),
            stroke_width: 2.0,
        });
        scene.indicator = Some(Indicator {
            text: "Native probe".into(),
            held_text: Some("HELD: SHIFT".into()),
            position: Point::new(clip.x + 240.0, clip.y + 80.0),
            style: LabelStyle::default(),
        });

        let mut current = Arc::new(scene);
        let first_started = Instant::now();
        backend.present(Arc::clone(&current))?;
        println!(
            "native_first_present elapsed_ns={}",
            first_started.elapsed().as_nanos()
        );
        process_metrics("first_present");

        let mut cursor_samples = Vec::with_capacity(MOTION_SAMPLES);
        for index in 0..MOTION_SAMPLES {
            let mut next = current.as_ref().clone();
            let x = clip.x + 100.0 + (index % 600) as f64;
            let y = clip.y + 100.0 + (index % 300) as f64;
            if let Some(cursor) = next.cursor_marker.as_mut() {
                cursor.center = Point::new(x, y);
            }
            current = Arc::new(next);
            let started = Instant::now();
            backend.present(Arc::clone(&current))?;
            cursor_samples.push(started.elapsed().as_nanos());
        }
        print_percentiles("native_cursor_move", &mut cursor_samples);

        let mut indicator_samples = Vec::with_capacity(MOTION_SAMPLES);
        for index in 0..MOTION_SAMPLES {
            let mut next = current.as_ref().clone();
            let x = clip.x + 100.0 + (index % 600) as f64;
            let y = clip.y + 100.0 + (index % 300) as f64;
            if let Some(indicator) = next.indicator.as_mut() {
                indicator.position = Point::new(x + 80.0, y + 28.0);
            }
            current = Arc::new(next);
            let started = Instant::now();
            backend.present(Arc::clone(&current))?;
            indicator_samples.push(started.elapsed().as_nanos());
        }
        print_percentiles("native_indicator_move", &mut indicator_samples);
        process_metrics("motion_steady");

        let dismiss_started = Instant::now();
        backend.dismiss()?;
        println!(
            "native_dismiss elapsed_ns={}",
            dismiss_started.elapsed().as_nanos()
        );
        process_metrics("dismissed");
        Ok(())
    }

    fn probe_input(backend: &MacOsBackend) -> Result<(), String> {
        let shift = Key::new("left_shift")?;
        let f20 = Key::new("f20")?;
        let mut samples = Vec::with_capacity(INPUT_SAMPLES);
        for _ in 0..INPUT_SAMPLES {
            let started = Instant::now();
            backend.send_keys(vec![
                (shift.clone(), KeyState::Down),
                (f20.clone(), KeyState::Down),
                (f20.clone(), KeyState::Up),
                (shift.clone(), KeyState::Up),
            ])?;
            samples.push(started.elapsed().as_nanos());
        }
        print_percentiles("native_key_batch", &mut samples);
        Ok(())
    }

    fn probe_scans(backend: &mut MacOsBackend, fixture_pid: u32) -> Result<(), String> {
        let mut request_id = 1_u64;
        for strategy in [
            UiScanStrategy::AxTree,
            UiScanStrategy::Vision,
            UiScanStrategy::Hybrid,
        ] {
            let _ = run_scan(backend, request_id, strategy, fixture_pid)?;
            request_id += 1;
            let mut first_samples = Vec::with_capacity(SCAN_SAMPLES);
            let mut total_samples = Vec::with_capacity(SCAN_SAMPLES);
            let mut last_targets = Vec::new();
            for _ in 0..SCAN_SAMPLES {
                let sample = run_scan(backend, request_id, strategy, fixture_pid)?;
                request_id += 1;
                first_samples.push(sample.first_partial_ns);
                total_samples.push(sample.total_ns);
                last_targets = sample.targets;
            }
            let name = match strategy {
                UiScanStrategy::AxTree => "ax",
                UiScanStrategy::Vision => "vision",
                UiScanStrategy::Hybrid => "hybrid",
            };
            print_percentiles(&format!("native_{name}_first_partial"), &mut first_samples);
            print_percentiles(&format!("native_{name}_terminal"), &mut total_samples);
            println!("native_{name}_targets count={}", last_targets.len());
            print_fixture_targets(name, &last_targets);
            process_metrics(&format!("{name}_steady"));
        }

        for _ in 0..50 {
            let _ = run_scan(backend, request_id, UiScanStrategy::Hybrid, fixture_pid)?;
            request_id += 1;
        }
        process_metrics("hybrid_after_50");
        Ok(())
    }

    struct ScanSample {
        first_partial_ns: u128,
        total_ns: u128,
        targets: Vec<UiTarget>,
    }

    fn run_scan(
        backend: &mut MacOsBackend,
        id: u64,
        strategy: UiScanStrategy,
        fixture_pid: u32,
    ) -> Result<ScanSample, String> {
        let request = UiScanRequest {
            id,
            timeout_ms: 2_500,
            bounds: None,
            roles: Vec::new(),
            max_depth: 50,
            visible_only: true,
            clickable_only: true,
            strategy,
            vision: VisionOptions::default(),
            app: Some(FocusedApp {
                bundle_id: "dev.keysteer.native-probe-fixture".into(),
                window_title: "KeySteer Native Probe Fixture".into(),
                process_id: fixture_pid,
            }),
        };
        let started = Instant::now();
        backend.request_ui_scan(request)?;
        let deadline = started + Duration::from_secs(12);
        let mut first_partial = None;
        let mut targets = Vec::new();
        loop {
            if Instant::now() >= deadline {
                return Err(format!("scan {id} exceeded the native probe deadline"));
            }
            let Some(event) = backend.poll(Duration::from_millis(100))? else {
                continue;
            };
            match event {
                BackendEvent::UiScanned(result) if result.id == id => {
                    targets.extend(result.targets);
                    if result.status == UiScanStatus::Partial {
                        first_partial.get_or_insert_with(|| started.elapsed().as_nanos());
                        continue;
                    }
                    if !matches!(result.status, UiScanStatus::Success) {
                        return Err(format!("scan {id} ended with {:?}", result.status));
                    }
                    let total_ns = started.elapsed().as_nanos();
                    return Ok(ScanSample {
                        first_partial_ns: first_partial.unwrap_or(total_ns),
                        total_ns,
                        targets,
                    });
                }
                BackendEvent::Input(_) => backend.dispose_key(KeyDisposition::Forward)?,
                _ => {}
            }
        }
    }

    fn print_fixture_targets(source: &str, targets: &[UiTarget]) {
        let mut fixture: Vec<_> = targets
            .iter()
            .filter(|target| target.name.starts_with("Fixture Button "))
            .collect();
        fixture.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.role.cmp(&right.role))
                .then_with(|| left.rect.x.total_cmp(&right.rect.x))
                .then_with(|| left.rect.y.total_cmp(&right.rect.y))
        });
        for target in fixture {
            println!(
                "native_{source}_target name_hex={} role_hex={} x={:.6} y={:.6} width={:.6} height={:.6}",
                hex_text(&target.name),
                hex_text(&target.role),
                target.rect.x,
                target.rect.y,
                target.rect.width,
                target.rect.height,
            );
        }
    }

    fn hex_text(text: &str) -> String {
        use std::fmt::Write as _;

        let mut encoded = String::with_capacity(text.len() * 2);
        for byte in text.as_bytes() {
            let _ = write!(encoded, "{byte:02x}");
        }
        encoded
    }

    fn print_percentiles(name: &str, samples: &mut [u128]) {
        samples.sort_unstable();
        let at = |percent: usize| {
            let index = ((samples.len() - 1) * percent).div_ceil(100);
            samples[index]
        };
        println!(
            "{name} samples={} p50={}ns p95={}ns p99={}ns",
            samples.len(),
            at(50),
            at(95),
            at(99)
        );
    }

    fn process_metrics(stage: &str) {
        let pid = std::process::id().to_string();
        let ps = Command::new("/bin/ps")
            .args(["-o", "rss=,thcount=", "-p", &pid])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unavailable".into());
        let footprint = Command::new("/usr/bin/vmmap")
            .args(["-summary", &pid])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .find(|line| line.trim_start().starts_with("Physical footprint:"))
                    .map(|line| line.trim().to_string())
            })
            .unwrap_or_else(|| "Physical footprint: unavailable".into());
        println!("native_metrics stage={stage} ps=\"{ps}\" {footprint}");
    }

    struct FixtureProcess {
        child: Child,
    }

    impl FixtureProcess {
        fn spawn() -> Result<Self, String> {
            let executable = std::env::current_exe()
                .map_err(|error| format!("cannot locate native probe executable: {error}"))?;
            let mut child = Command::new(executable)
                .arg("--fixture")
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|error| format!("cannot start AppKit fixture: {error}"))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| "AppKit fixture stdout is unavailable".to_string())?;
            let mut line = String::new();
            BufReader::new(stdout)
                .read_line(&mut line)
                .map_err(|error| format!("cannot read AppKit fixture readiness: {error}"))?;
            if !line.starts_with("fixture_ready") {
                let _ = child.kill();
                return Err(format!("AppKit fixture did not become ready: {line:?}"));
            }
            Ok(Self { child })
        }

        fn pid(&self) -> u32 {
            self.child.id()
        }

        fn stop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    impl Drop for FixtureProcess {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn run_fixture() -> Result<(), String> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "AppKit fixture must run on the main thread".to_string())?;
        let application = NSApplication::sharedApplication(mtm);
        application.setActivationPolicy(NSApplicationActivationPolicy::Regular);

        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(760.0, 520.0));
        let window = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setTitle(&NSString::from_str("KeySteer Native Probe Fixture"));
        let content = NSView::initWithFrame(NSView::alloc(mtm), frame);
        for index in 0..24 {
            let button = NSButton::new(mtm);
            button.setTitle(&NSString::from_str(&format!("Fixture Button {index:02}")));
            button.setFrame(NSRect::new(
                NSPoint::new(
                    24.0 + f64::from(index % 4) * 180.0,
                    32.0 + f64::from(index / 4) * 72.0,
                ),
                NSSize::new(150.0, 36.0),
            ));
            content.addSubview(&button);
        }
        window.setContentView(Some(&content));
        window.center();
        window.makeKeyAndOrderFront(None);
        application.finishLaunching();
        application.activate();
        println!("fixture_ready pid={}", std::process::id());
        std::io::stdout()
            .flush()
            .map_err(|error| format!("cannot flush fixture readiness: {error}"))?;
        application.run();
        Ok(())
    }
}
