//! Command-line parsing and user-facing help.

use std::path::PathBuf;
use std::process::ExitCode;

/// A Windows GUI-subsystem binary has no console when started by double-click.
/// Attach one only when command-line options explicitly request CLI behaviour.
#[cfg(target_os = "windows")]
pub fn prepare_console_for_cli() {
    if std::env::args_os().len() <= 1 {
        return;
    }
    crate::platform::prepare_console_for_cli();
}

#[cfg(not(target_os = "windows"))]
pub fn prepare_console_for_cli() {}

#[derive(Debug)]
pub(crate) struct CliOptions {
    pub(crate) config: Option<PathBuf>,
    pub(crate) check_only: bool,
    pub(crate) dump_config: bool,
    pub(crate) doctor: bool,
    #[cfg(target_os = "windows")]
    internal_wechat_ocr: Option<(PathBuf, PathBuf, PathBuf)>,
}

pub fn run_cli() -> ExitCode {
    let log_path = match super::logging::init() {
        Ok(path) => Some(path.to_path_buf()),
        Err(error) => {
            eprintln!("KeySteer: {error}");
            None
        }
    };
    super::logging::install_panic_hook();
    match parse_args().and_then(|args| {
        args.map_or(Ok(()), |args| {
            #[cfg(target_os = "windows")]
            if let Some((bridge, component, runtime)) = args.internal_wechat_ocr {
                return crate::platform::run_internal_wechat_ocr_helper(bridge, component, runtime);
            }
            super::bootstrap::run(args)
        })
    }) {
        Ok(()) => {
            crate::log_info!("session", "session ended normally");
            super::logging::flush();
            ExitCode::SUCCESS
        }
        Err(error) => {
            super::logging::report_error("cli", &error);
            if let Some(log_path) = log_path {
                eprintln!("KeySteer: diagnostic log: {}", log_path.display());
            }
            super::logging::flush();
            ExitCode::FAILURE
        }
    }
}

fn parse_args() -> Result<Option<CliOptions>, String> {
    let mut args = CliOptions {
        config: None,
        check_only: false,
        dump_config: false,
        doctor: false,
        #[cfg(target_os = "windows")]
        internal_wechat_ocr: None,
    };
    let mut iter = std::env::args().skip(1);

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("KeySteer {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-c" | "--config" => {
                let path = iter
                    .next()
                    .ok_or_else(|| format!("{arg} requires a path"))?;
                args.config = Some(PathBuf::from(path));
            }
            "--check" => args.check_only = true,
            "--dump-config" => args.dump_config = true,
            "--doctor" => args.doctor = true,
            #[cfg(target_os = "windows")]
            "--internal-wechat-ocr-helper" => {
                let bridge = iter
                    .next()
                    .ok_or_else(|| "internal helper requires a bridge path".to_string())?;
                let component = iter
                    .next()
                    .ok_or_else(|| "internal helper requires a component path".to_string())?;
                let runtime = iter
                    .next()
                    .ok_or_else(|| "internal helper requires a runtime path".to_string())?;
                args.internal_wechat_ocr = Some((
                    PathBuf::from(bridge),
                    PathBuf::from(component),
                    PathBuf::from(runtime),
                ));
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Some(args))
}

fn print_help() {
    println!(
        "\
KeySteer {version} — keyboard-driven mouse control

USAGE:
    keysteer [OPTIONS]

OPTIONS:
    -c, --config <PATH>  Use an explicit keysteer.<name>.toml file
        --check          Validate the configuration and exit
        --doctor         Report whether the program can work here, and exit
        --dump-config    Print the effective configuration and exit
    -h, --help           Show this help
    -V, --version        Show the version

MODES:
    idle             Silent resting state; only listens for the keys that
                     launch a mode, so the program stays out of the way
    normal           Move the pointer, click and scroll; entry point to the
                     three targeting modes below
    grid             Full-screen labelled coordinate grid
    recursive_grid   Recursively subdividing grid
    ui_hint          Labels every clickable element

LAUNCHING (from idle):
    Primary+E            normal          Primary is Cmd on macOS,
                                        Ctrl on Windows and Linux

IN NORMAL MODE:
    h j k l          move the pointer
    caps/left-shift  precision / slow; v or b is fast
    ; ' right-shift left / right / middle click
    n                toggle left-button drag
    m ,              scroll down / up
    g f Primary+F    grid / recursive_grid / ui_hint
    t y i u          home / end / page_up / page_down
    q or esc         back to idle

    Everything is rebindable; see the `[normal.bindings]` section.

CONFIGURATION:
    Configuration is optional. Without one, KeySteer uses built-in defaults.
    It selects the first file, sorted by name, matching:
      keysteer.<name>.toml

    A packaged macOS app reads and writes configuration and keysteer.log in:
      ~/Library/Application Support/KeySteer/
    Bare binaries and Windows portable builds keep them beside the executable.
    With -c, a bare file name is resolved beside the executable; a relative or
    absolute path is used as written.

    `[key_aliases]` applies globally; `[key_aliases.windows|macos|linux]`
    overrides it on one platform. Generic modifiers match both physical sides;
    `left_` / `right_` require one side.

    Built for the {backend} backend.",
        version = env!("CARGO_PKG_VERSION"),
        backend = crate::platform::backend_name(),
    );
    if let Some(path) = super::logging::path() {
        println!(
            "\nDIAGNOSTICS:\n    Always-on runtime log: {}",
            path.display()
        );
    }
}
