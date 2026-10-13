//! Paani: an open-source client for Roblox places.

mod app;
mod blit;
mod fly;
mod physics;

use std::path::PathBuf;

use anyhow::{Context, Result};
use winit::event_loop::EventLoop;

fn main() -> Result<()> {
    let path: PathBuf = std::env::args_os().nth(1).map(Into::into).context("usage: paani <place.rbxl>")?;
    let dom = kahua_io::read_file(&path).with_context(|| format!("failed to open {}", path.display()))?;
    let name = path.file_stem().unwrap_or_default().to_string_lossy();

    let event_loop = EventLoop::new()?;
    let mut app = app::App::new(dom, format!("{name} - Paani"));
    event_loop.run_app(&mut app)?;
    app.finish()
}
