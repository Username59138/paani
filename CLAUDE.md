# Paani

Open-source Roblox place client in Rust. Companion to Kahua Studio (`../kahua`).

- Reuses Kahua crates from git: `kahua-io` (place files, rbx-dom), `kahua-render` (wgpu renderer).
  Graphics improvements belong in `kahua-render` so the studio gets them too. To edit both at once,
  create an untracked `.cargo/config.toml` with
  `[patch."https://github.com/Username59138/kahua"]` + `kahua-io = { path = "../kahua/crates/kahua-io" }`
  (same for kahua-render); run `cargo update -p kahua-io -p kahua-render` after Kahua pushes.
- `src/app.rs` window + GPU + event loop (winit 0.30, wgpu 30); `src/blit.rs` copies Kahua's
  offscreen target (already sRGB-encoded) to a non-sRGB surface; `src/fly.rs` free camera;
  `src/physics.rs` rapier 0.36 (own glam version, convert via arrays): welded parts = one body,
  anchored assembly = fixed body. Moved parts re-upload the whole scene with `set_scene` (fine for now).
- Test places: `examples/demo.rbxl` (Kahua's `demo_place` example), `examples/physics.rbxl`
  (`cargo run --example physics_demo -- examples/physics.rbxl`; `cargo test` simulates it).
- Usage limits are tight: one feature per session, keep changes small.

## Plan
1. Engine (offline): ~~render places~~ → ~~physics~~ → character → Luau (mlua) → GUI
2. Client: home screen, settings, graphics presets, profiles, stats (mockups exist)
3. Online: Roblox servers, server browser. No cheat features.
