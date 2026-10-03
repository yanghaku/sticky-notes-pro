#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod editor;
mod instance;
mod model;
mod persistence;
mod platform;
mod shortcuts;
mod text_layout;
mod worker;

slint::include_modules!();

fn main() -> anyhow::Result<()> {
    app::run()
}
