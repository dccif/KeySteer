fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=assets/icons/keysteer.ico");
    println!("cargo:rerun-if-changed=src/platform/windows/compositor_clock.c");
    println!("cargo:rerun-if-changed=src/platform/macos/vision_bridge.m");
    println!("cargo:rerun-if-changed=src/platform/macos/autostart_bridge.m");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        compile_windows_resources()?;
    }
    if target_os == "macos" {
        compile_macos_bridge();
    }
    Ok(())
}

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
