#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests.
//!
//! These check the properties that hold *across* modules: that the shipped
//! configuration stays valid, that the five modes fit together, and that a
//! plugin is treated exactly like a built-in mode.

use keysteer::api::{
    Appearance, Binding, Command, Direction, HostContext, KeyChord, Mode, ModeEvent, ModeId, Point,
    Rect, Screen, UiScanStrategy,
};
use keysteer::config::{Config, LifecycleAction, TargetingLifecycle};
use keysteer::{Engine, modes, plugins};

fn screens() -> Vec<Screen> {
    vec![Screen {
        bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
        work_area: Rect::new(0.0, 0.0, 1920.0, 1055.0),
        is_primary: true,
        scale: 2.0,
        name: Some("built-in".into()),
    }]
}

fn shipped() -> Config {
    Config::parse(include_str!("../keysteer.default.toml"))
        .expect("keysteer.default.toml should parse")
}

fn shipped_document() -> toml::Value {
    toml::from_str(include_str!("../keysteer.default.toml"))
        .expect("keysteer.default.toml should be valid TOML")
}

#[test]
fn the_shipped_config_parses_and_validates() {
    let config = shipped();
    config
        .validate()
        .expect("keysteer.default.toml should validate");
    assert_eq!(config.ui_hint.strategy, UiScanStrategy::Hybrid);
    assert_eq!(config.ui_hint.ui.font_size, 17);
    assert_eq!(config.ui_hint.ui.padding_x, -1);
    assert_eq!(config.ui_hint.ui.padding_y, -1);
    assert_eq!(config.ui_hint.label_y_offset, -8);
}

#[test]
fn the_shipped_config_matches_the_embedded_product_defaults() {
    assert_eq!(shipped(), Config::default());
}

#[test]
fn the_shipped_targeting_lifecycles_are_explicit() {
    let config = shipped();
    assert_eq!(
        config.ui_hint.lifecycle,
        TargetingLifecycle {
            after_finish: LifecycleAction::Mode(ModeId::normal()),
            after_click: LifecycleAction::Mode(ModeId::normal()),
        }
    );
    assert_eq!(
        config.grid.lifecycle,
        TargetingLifecycle {
            after_finish: LifecycleAction::Mode(ModeId::normal()),
            after_click: LifecycleAction::Finish,
        }
    );
    assert_eq!(
        config.recursive_grid.lifecycle,
        TargetingLifecycle {
            after_finish: LifecycleAction::Keep,
            after_click: LifecycleAction::Keep,
        }
    );
}

#[test]
fn the_macos_bridge_does_not_require_clang_availability_runtime() {
    let bridge = include_str!("../src/platform/macos/vision_bridge.m");
    assert!(
        !bridge.contains("if (@available"),
        "@available introduces __isPlatformVersionAtLeast, which Rust's -nodefaultlibs link omits"
    );
}

#[test]
fn macos_autostart_registers_the_keysteer_bundle_instead_of_open() {
    let bridge = include_str!("../src/platform/macos/autostart_bridge.m");
    let rust = include_str!("../src/platform/macos/autostart.rs");
    assert!(bridge.contains("SMAppService.mainAppService"));
    assert!(bridge.contains("registerAndReturnError"));
    assert!(!rust.contains("<string>/usr/bin/open</string>"));
    assert!(!rust.contains("document_for_executable"));
    assert!(!bridge.contains("/usr/bin/open"));
}

#[test]
fn the_shipped_config_reaches_normal_from_idle() {
    let config = shipped();
    assert!(
        config
            .hotkeys
            .values()
            .any(|b| b.mode() == Some(&ModeId::normal())),
        "keysteer.default.toml must bind a way into normal: {:?}",
        config.hotkeys
    );
}

#[test]
fn the_shipped_config_launchers_are_platform_neutral() {
    // Regression: `alt+e` never fired on macOS, because Option+E types a
    // dead-key accent that the OS consumes before we see it.
    // Inspect the portable source spelling: Config parsing intentionally
    // resolves `primary` to the current platform's concrete modifier.
    let document = shipped_document();
    let hotkeys = document["hotkeys"].as_table().unwrap();
    for chord in hotkeys.keys() {
        assert!(
            chord.contains("primary"),
            "launcher {chord:?} should use `primary` so one file works everywhere"
        );
    }
}

#[test]
fn the_shipped_config_has_no_macos_option_letter_chords() {
    // The specific trap: a bare Option/Alt plus a letter.
    let document = shipped_document();
    let tables = [
        document["hotkeys"].as_table().unwrap(),
        document["normal"]["bindings"].as_table().unwrap(),
    ];
    for table in tables {
        for chord in table.keys() {
            let lower = chord.to_lowercase();
            let uses_alt = lower.contains("alt") || lower.contains("option");
            if !uses_alt {
                continue;
            }
            // Alt is only safe when combined with cmd/ctrl, which stops macOS
            // from treating it as text composition.
            assert!(
                lower.contains("primary") || lower.contains("cmd") || lower.contains("ctrl"),
                "{chord:?} uses a bare Alt+letter, which composes text on macOS"
            );
        }
    }
}

#[test]
fn the_shipped_config_validates_without_platform_warnings() {
    let config = shipped();
    config
        .validate()
        .expect("keysteer.default.toml should validate");
    assert!(config.platform_warnings().is_empty());
    assert!(config.deprecation_warnings().is_empty());
    for chord in config.hotkeys.keys().chain(config.normal.bindings.keys()) {
        let parsed = KeyChord::parse(chord).expect("shipped chords must parse");
        assert!(
            !parsed.keys().is_empty(),
            "{chord:?} resolved to an empty chord"
        );
    }
}

#[test]
fn the_shipped_config_reaches_every_targeting_mode_from_normal() {
    let config = shipped();
    let reachable: Vec<&str> = config
        .normal
        .bindings
        .values()
        .filter_map(|b| b.mode())
        .map(|id| id.as_str())
        .collect();

    for mode in ["grid", "recursive_grid", "ui_hint"] {
        assert!(
            reachable.contains(&mode),
            "{mode} is unreachable from normal: {reachable:?}"
        );
    }
}

#[test]
fn the_shipped_targeting_modes_bind_primary_q_to_normal() {
    let document = shipped_document();
    for mode in ["grid", "recursive_grid", "ui_hint"] {
        let table = document[mode]["bindings"].as_table().unwrap();
        assert_eq!(table["primary+q"].as_str(), Some("normal"));
    }
}

#[test]
fn the_shipped_config_uses_no_action_prefix() {
    // The whole point of the binding vocabulary: verbs stand alone.
    let text = include_str!("../keysteer.default.toml");
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        assert!(
            !line.contains("\"action "),
            "keysteer.default.toml should not use an `action` prefix: {line}"
        );
    }
}

#[test]
fn defaults_and_shipped_config_agree_on_mode_availability() {
    let ids: Vec<ModeId> = modes::built_in(&shipped()).iter().map(|m| m.id()).collect();
    assert_eq!(ids.len(), 5, "expected all five modes: {ids:?}");
}

#[test]
fn idle_and_default_normal_let_unbound_keys_reach_the_focused_app() {
    for mode in modes::built_in(&Config::default()) {
        let expected = !matches!(mode.id().as_str(), "idle" | "normal");
        assert_eq!(
            mode.captures_keyboard(),
            expected,
            "{} has the wrong capture policy",
            mode.id()
        );
    }
}

#[test]
fn plugins_register_through_the_same_path_as_built_ins() {
    let config = Config::default();
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in modes::built_in(&config) {
        engine.register(mode);
    }
    for plugin in plugins::bundled(&config).unwrap() {
        engine.register_plugin_dyn(plugin).unwrap();
    }

    let ids: Vec<String> = engine.registered_modes().map(|i| i.to_string()).collect();
    for expected in ["idle", "normal", "grid", "plugin:screen-selector"] {
        assert!(
            ids.contains(&expected.to_string()),
            "{expected} missing from {ids:?}"
        );
    }
}

#[test]
fn a_plugin_chord_is_bound_in_normal_like_any_other_mode() {
    // A plugin's suggested chord must land in the table the user works in.
    let config = Config::default();
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in modes::built_in(&config) {
        engine.register(mode);
    }
    for plugin in plugins::bundled(&config).unwrap() {
        engine.register_plugin_dyn(plugin).unwrap();
    }

    let bound = engine.bindings_in(&ModeId::normal());
    assert!(
        bound.iter().any(|(_, binding)| {
            matches!(
                binding,
                Binding::Invoke { verb, args }
                    if verb == "screen" && args == &["next"]
            )
        }),
        "the plugin chord should invoke its exported verb from normal: {bound:?}"
    );
}

#[test]
fn a_users_binding_beats_a_plugins_suggestion() {
    // Config always wins, so a plugin can never steal a key the user assigned.
    let mut config = Config::default();
    config
        .normal
        .bindings
        .insert("alt+s".into(), Binding::parse("move_left").unwrap());

    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in modes::built_in(&config) {
        engine.register(mode);
    }
    for plugin in plugins::bundled(&config).unwrap() {
        engine.register_plugin_dyn(plugin).unwrap();
    }

    let bound = engine.bindings_in(&ModeId::normal());
    let alt_s: Vec<&Binding> = bound
        .iter()
        .filter(|(chord, _)| chord == "alt+s")
        .map(|(_, b)| b)
        .collect();
    assert_eq!(alt_s, vec![&Binding::Move(Direction::Left)]);
}

#[test]
fn every_targeting_mode_returns_to_idle_on_escape() {
    let config = Config::default();
    let screens = screens();
    let palette = config.palette(Appearance::Dark);

    for mut mode in modes::built_in(&config) {
        // idle has nowhere to go; normal's escape is the engine's job.
        if mode.id() == ModeId::idle() || mode.id() == ModeId::normal() {
            continue;
        }
        let ctx = HostContext {
            screens: &screens,
            cursor: Point::new(960.0, 540.0),
            focused_app: None,
            palette: &palette,
            config: &config,
        };

        mode.handle(&ModeEvent::Activated { previous: None }, &ctx);
        let out = mode.handle(
            &ModeEvent::Key {
                key: "esc".parse().unwrap(),
                state: keysteer::api::KeyState::Down,
                repeat: false,
            },
            &ctx,
        );
        assert!(
            out.contains(&Command::SwitchMode(ModeId::idle())),
            "{} does not return to idle on escape: {out:?}",
            mode.id()
        );
    }
}

#[test]
fn a_grid_overlay_covers_the_active_screen_at_any_scale() {
    let config = Config::default();
    let screens = screens();
    let palette = config.palette(Appearance::Dark);
    let ctx = HostContext {
        screens: &screens,
        cursor: Point::new(960.0, 540.0),
        focused_app: None,
        palette: &palette,
        config: &config,
    };

    let mut grid = modes::GridMode::new(&config);
    let out = grid.handle(&ModeEvent::Activated { previous: None }, &ctx);

    let scene = out
        .iter()
        .find_map(|c| match c {
            Command::ShowOverlay(s) => Some(s),
            _ => None,
        })
        .expect("grid should draw on activation");

    // Cells must span the display, not a fraction of it. Subdivision
    // accumulates floating-point error, so compare within a pixel.
    let covered = scene
        .shapes
        .iter()
        .filter_map(|s| match s {
            keysteer::api::OverlayShape::Rect { rect, .. } => Some(*rect),
            _ => None,
        })
        .reduce(|a, b| a.union(&b))
        .expect("grid should draw cells");

    let expected = screens[0].bounds;
    for (label, got, want) in [
        ("x", covered.x, expected.x),
        ("y", covered.y, expected.y),
        ("width", covered.width, expected.width),
        ("height", covered.height, expected.height),
    ] {
        assert!(
            (got - want).abs() < 1.0,
            "{label}: got {got}, expected {want}"
        );
    }
}

#[test]
fn every_mode_has_a_binding_table_including_plugins() {
    // Uniform configuration is what makes plugins first-class.
    let mut config = Config::default();
    config.plugin_modes.insert(
        "plugin:screen-selector".into(),
        keysteer::config::PluginModeConfig {
            bindings: [("esc".to_string(), Binding::Escape)].into_iter().collect(),
            ..Default::default()
        },
    );

    for mode in ["idle", "normal", "grid", "recursive_grid", "ui_hint"] {
        assert!(config.bindings_for(mode).is_some(), "{mode} has no table");
    }
    assert!(config.bindings_for("plugin:screen-selector").is_some());
    config.validate().unwrap();
}

#[test]
fn windows_visual_capture_keeps_one_barrier_and_an_unscaled_copy_path() {
    let gpu = include_str!("../src/platform/windows/gpu_overlay.rs");
    let worker = include_str!("../src/platform/windows/overlay_worker.rs");
    let native = include_str!("../src/platform/windows/native/mod.rs");

    assert!(
        !gpu.contains("WaitForCommitCompletion"),
        "ordinary overlay dismiss must not wait for the compositor"
    );
    assert_eq!(
        worker.matches("native::wait_for_dwm_frame()").count(),
        1,
        "capture must have exactly one explicit DWM barrier"
    );
    assert!(
        native.contains("BitBlt(") && native.contains("StretchBlt("),
        "capture must retain separate unscaled and scaled GDI paths"
    );
    assert!(
        native.contains("source_width == self.dimensions.width_i32()")
            && native.contains("source_height == self.dimensions.height_i32()"),
        "the BitBlt path must be restricted to exact-size copies"
    );
}
