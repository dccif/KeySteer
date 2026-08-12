fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icons/keysteer.ico");
    println!("cargo:rerun-if-changed=src/platform/windows/compositor_clock.c");
    println!("cargo:rerun-if-changed=src/platform/macos/vision_bridge.m");
    println!("cargo:rerun-if-changed=src/platform/macos/autostart_bridge.m");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rustc-env=KEYSTEER_BUILD_DATE={}", build_date()?);

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        compile_windows_resources()?;
    }
    if target_os == "macos" {
        compile_macos_bridge();
    }
    Ok(())
}

fn build_date() -> Result<String, Box<dyn std::error::Error>> {
    let timestamp = match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(value) => value.parse::<u64>()?,
        Err(_) => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
    };
    let (year, month, day) = civil_date(timestamp / 86_400);
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

// Convert days since 1970-01-01 to a Gregorian date without adding a build
// dependency. SOURCE_DATE_EPOCH keeps release builds reproducible when set.
fn civil_date(days_since_epoch: u64) -> (i64, i64, i64) {
    let days = days_since_epoch as i64 + 719_468;
    let era = days / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(windows)]
fn compile_windows_resources() -> Result<(), Box<dyn std::error::Error>> {
    // Official Windows artifacts are built on Windows. Cross-platform
    // `cargo check --target ...` runs without a Windows resource compiler.
    if !std::env::var("HOST").is_ok_and(|host| host.contains("windows")) {
        println!(
            "cargo:warning=skipping Windows icon while cross-checking from a non-Windows host"
        );
        return Ok(());
    }

    cc::Build::new()
        .file("src/platform/windows/compositor_clock.c")
        .warnings(true)
        .compile("keysteer_windows");

    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/icons/keysteer.ico")
        .set("ProductName", "KeySteer")
        // Explorer and Task Manager's Startup Apps surface this field. Keep
        // it compact; the longer product description belongs in documentation
        // and Cargo metadata, not system lists.
        .set("FileDescription", "KeySteer")
        .set("InternalName", "keysteer.exe")
        .set("OriginalFilename", "keysteer.exe");
    resource.compile()?;
    Ok(())
}

#[cfg(not(windows))]
fn compile_windows_resources() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:warning=skipping Windows resources on a non-Windows host");
    Ok(())
}

#[cfg(target_os = "macos")]
fn compile_macos_bridge() {
    // `cargo check --target ...-apple-darwin` can still type-check the Rust
    // backend from another host. Only a macOS host has the SDK and Objective-C
    // compiler needed to build and link this bridge for a release artifact.
    if !std::env::var("HOST").is_ok_and(|host| host.contains("apple-darwin")) {
        println!(
            "cargo:warning=skipping macOS Objective-C bridge while checking from a non-macOS host"
        );
        return;
    }

    const MIN_MACOS: &str = "14.0";
    cc::Build::new()
        .file("src/platform/macos/vision_bridge.m")
        .file("src/platform/macos/autostart_bridge.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag(format!("-mmacosx-version-min={MIN_MACOS}"))
        .compile("keysteer_vision");
    println!("cargo:rustc-link-arg=-mmacosx-version-min={MIN_MACOS}");
    for framework in [
        "Foundation",
        "CoreGraphics",
        "ScreenCaptureKit",
        "ServiceManagement",
        "Vision",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

#[cfg(not(target_os = "macos"))]
fn compile_macos_bridge() {
    println!("cargo:warning=skipping macOS Objective-C bridge on a non-macOS host");
}
