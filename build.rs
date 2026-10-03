fn main() -> Result<(), Box<dyn std::error::Error>> {
    slint_build::compile("ui/app.slint")?;
    embed_windows_icon()?;
    Ok(())
}

/// Embed `asserts/app.ico` into the executable. The target is read at run time:
/// `#[cfg(target_os)]` in a build script describes the host, so gating on it
/// would drop the icon whenever cross-compiling.
fn embed_windows_icon() -> Result<(), Box<dyn std::error::Error>> {
    // slint-build's rerun-if-changed list already disables cargo's package-wide rescan.
    println!("cargo:rerun-if-changed=asserts/app.ico");
    if !std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        return Ok(());
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("asserts/app.ico");
    resource
        .compile()
        .map_err(|error| format!("cannot embed asserts/app.ico into the executable: {error}"))?;
    Ok(())
}
