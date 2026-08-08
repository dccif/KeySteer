//! Application bootstrap and diagnostics.

use crate::api::Key;
use crate::config::{Config, ConfigStore};
use crate::{Engine, modes, platform, plugins};

use super::cli::CliOptions;

pub(crate) fn run(args: CliOptions) -> Result<(), String> {
    let rediscover_config_on_reload = args.config.is_none();
    let (config, config_path, loaded_from_file) = match &args.config {
        Some(name) => {
            if !name
                .file_name()
                .is_some_and(Config::is_portable_config_name)
            {
                return Err(format!(
                    "config must be named keysteer.<name>.toml: {}",
                    name.display()
                ));
            }
            let path = crate::app::paths::explicit_config_file(name)?;
            (
                Config::load(&path).map_err(|e| e.to_string())?,
                Some(path),
                true,
            )
        }
        None => match Config::discover() {
            Some(path) => match Config::load(&path) {
                Ok(config) => (config, Some(path), true),
                Err(error) => {
                    crate::app::logging::report_error(
                        "config",
                        format!(
                            "could not apply {}; using built-in defaults: {error}",
                            path.display()
                        ),
                    );
                    (Config::default(), Config::default_write_path(), false)
                }
            },
            None => (Config::default(), Config::default_write_path(), false),
        },
    };
    crate::app::logging::set_non_error_enabled(config.debug.enabled);
    if loaded_from_file {
        let path = config_path.as_deref().ok_or_else(|| {
            "configuration loader reported a file without retaining its path".to_string()
        })?;
        crate::log_info!("config", "configuration loaded from {}", path.display());
    } else if let Some(path) = config_path.as_deref() {
        crate::log_info!(
            "config",
            "using built-in configuration; write path is {}",
            path.display()
        );
    } else {
        crate::log_info!(
            "config",
            "using built-in configuration; data directory unavailable, configuration writes disabled"
        );
    }

    log_debug_configuration(&config, config_path.as_deref());

    if args.dump_config {
        print!("{}", config.to_toml());
        return Ok(());
    }

    if config.debug.enabled {
        for warning in config
            .deprecation_warnings()
            .into_iter()
            .chain(config.platform_warnings())
        {
            crate::report_warning!("config", "{warning}");
        }
    }

    if args.check_only {
        config.validate().map_err(|e| e.to_string())?;
        println!("ok");
        return Ok(());
    }
    if args.doctor {
        return doctor(&config);
    }

    let mut backend = platform::backend()?;
    let mut engine = Engine::new(config.clone(), backend.appearance());
    if let Some(config_path) = config_path {
        let store = ConfigStore::open(config_path, &config).map_err(|e| e.to_string())?;
        if rediscover_config_on_reload {
            if let Some(directory) = crate::app::paths::data_dir() {
                engine.attach_discovered_config_store(store, directory);
            } else {
                engine.attach_config_store(store);
            }
        } else {
            engine.attach_config_store(store);
        }
    }

    for mode in modes::built_in(&config) {
        engine.register(mode);
    }
    for plugin in plugins::bundled(&config)? {
        if let Err(error) = engine.register_plugin_dyn(plugin) {
            crate::report_warning!("plugin", "skipping plugin: {error}");
        }
    }

    engine.run(backend.as_mut())
}

fn log_debug_configuration(config: &Config, config_path: Option<&std::path::Path>) {
    if !config.debug.enabled {
        return;
    }

    let config_path = config_path
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<built-in; writes disabled>".to_string());
    crate::app::logging::debug_args(
        "config",
        format_args!(
            "debug enabled; config={} backend={}",
            config_path,
            platform::backend_name()
        ),
    );
    if let Ok(primary) = Key::new_with_aliases("primary", config.resolved_key_aliases()) {
        crate::app::logging::debug_args(
            "config",
            format_args!("primary resolves to {primary:?} for this configuration"),
        );
    }
    crate::app::logging::debug_args(
        "config",
        format_args!(
            "categories: keys={} actions={} modes={} backend={} pointer={} motion={} overlay={} timers={}",
            config.debug.keys,
            config.debug.actions,
            config.debug.modes,
            config.debug.backend,
            config.debug.pointer,
            config.debug.motion,
            config.debug.overlay,
            config.debug.timers
        ),
    );
}

fn doctor(config: &Config) -> Result<(), String> {
    println!("KeySteer {}", env!("CARGO_PKG_VERSION"));
    println!("backend: {}", platform::backend_name());
    if let Some(path) = crate::app::logging::path() {
        println!("log:     {}", path.display());
    }

    let backend = platform::backend()?;
    let keyboard_ok = backend.keyboard_available();
    println!(
        "keyboard: {}",
        if keyboard_ok {
            "available"
        } else {
            "UNAVAILABLE"
        }
    );

    let screens = backend.screens().unwrap_or_else(|error| {
        crate::app::logging::report_error("doctor", format!("cannot enumerate screens: {error}"));
        Vec::new()
    });
    println!("screens:  {} detected", screens.len());
    for (index, screen) in screens.iter().enumerate() {
        println!(
            "  #{index}: {:?} {}x{} at {},{} scale {:.2}{}",
            screen.name.as_deref().unwrap_or("unnamed"),
            screen.bounds.width,
            screen.bounds.height,
            screen.bounds.x,
            screen.bounds.y,
            screen.scale,
            if screen.is_primary { " primary" } else { "" }
        );
    }
    println!("appearance: {:?}", backend.appearance());
    if let Ok(Some(app)) = backend.focused_app() {
        println!(
            "foreground: {} (pid {}, title {:?})",
            app.bundle_id, app.process_id, app.window_title
        );
    }
    #[cfg(target_os = "windows")]
    {
        println!("overlay: layered RGBA, topmost, click-through");
        println!("UI scan: Windows UI Automation control view");
        println!("hook: low-level keyboard handshake and coalesced pointer tracking");
    }

    let launchers: Vec<String> = config
        .hotkeys
        .iter()
        .filter_map(|(chord, binding)| binding.mode().map(|id| format!("  {chord}  ->  {id}")))
        .collect();
    if launchers.is_empty() {
        println!("\nNo mode launchers are configured, so nothing can be started.");
    } else {
        println!("\nMode launchers:");
        for line in launchers {
            println!("{line}");
        }
    }

    if let Some(reason) = backend.keyboard_unavailable_reason() {
        println!("\n{reason}");
        return Err("the keyboard is unavailable".into());
    }
    println!("\nEverything looks fine.");
    Ok(())
}
