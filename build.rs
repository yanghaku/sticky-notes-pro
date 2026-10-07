fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    slint_build::compile("ui/app.slint")?;
    embed_windows_resources()?;
    Ok(())
}

/// Embed the icon and Common Controls v6 manifest. The target is read at run time:
/// `#[cfg(target_os)]` in a build script describes the host, so gating on it
/// would drop these resources whenever cross-compiling.
fn embed_windows_resources() -> Result<(), Box<dyn std::error::Error>> {
    // slint-build's rerun-if-changed list already disables cargo's package-wide rescan.
    println!("cargo:rerun-if-changed=asserts/app.ico");
    println!("cargo:rerun-if-changed=asserts/app.manifest");
    if !std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        return Ok(());
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("asserts/app.ico");
    resource.set_manifest_file("asserts/app.manifest");
    resource
        .compile()
        .map_err(|error| format!("cannot embed Windows icon and manifest: {error}"))?;
    Ok(())
}
