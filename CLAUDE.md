# Paani

Open-source Roblox place client in Rust. Companion to Kahua Studio (`../kahua`).

- Reuses Kahua crates from git: `kahua-io` (place files, rbx-dom), `kahua-render` (wgpu renderer).
  Graphics improvements belong in `kahua-render` so the studio gets them too. To edit both at once,
  create an untracked `.cargo/config.toml` with
  `[patch."https://github.com/Username59138/kahua"]` + `kahua-io = { path = "../kahua/crates/kahua-io" }`
  (same for kahua-render); run `cargo update -p kahua-io -p kahua-render` after Kahua pushes.
- `src/app.rs` window + GPU + event loop (winit 0.30, wgpu 30); `src/blit.rs` copies Kahua's
  offscreen target (already sRGB-encoded) to a non-sRGB surface; `src/camera.rs` Roblox orbit
  camera + free camera; `src/input.rs` key/mouse state;
  `src/physics.rs` rapier 0.36 (own glam version, convert via arrays): welded parts = one body,
  anchored assembly = fixed body. 240 Hz steps and stiff contacts (Roblox gravity is strong).
  The whole scene is re-uploaded every frame with `set_scene` (fine for now, optimize later).
- `src/rig.rs` builds the character into the DOM like Roblox (StarterCharacter or R6 rig, Humanoid
  with StarterPlayer settings, Player in Players). `src/character.rs` moves it like a Humanoid:
  hovers on floor raycasts (capsule only above STEP_HEIGHT), reads WalkSpeed/Jump* from the
  Humanoid every frame. Not yet: writing the character's CFrame back to the DOM, R15, animations.
- Test places: `examples/demo.rbxl` (Kahua's `demo_place` example), `examples/physics.rbxl`
  (`cargo run --example physics_demo -- examples/physics.rbxl`; `cargo test` simulates it).
- Usage limits are tight: one feature per session, keep changes small.

## Plan
1. Engine (offline): ~~render places~~ → ~~physics~~ → ~~character~~ → Luau (mlua) → GUI
2. Client: home screen, settings, graphics presets, profiles, stats (mockups exist)
3. Online: Roblox servers, server browser. No cheat features.
