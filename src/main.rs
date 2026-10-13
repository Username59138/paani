//! Paani: an open-source client for Roblox places.

mod app;
mod blit;
mod camera;
mod character;
mod input;
mod luau;
mod physics;
mod rig;

use std::path::PathBuf;

use anyhow::{Context, Result};
use winit::event_loop::EventLoop;

fn main() -> Result<()> {
    let path: PathBuf = std::env::args_os().nth(1).map(Into::into).context("usage: paani <place.rbxl>")?;
    let name = path.file_stem().unwrap_or_default().to_string_lossy();
    let mut app = app::App::load(&path, format!("{name} - Paani"))?;

    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut app)?;
    app.finish()
}
