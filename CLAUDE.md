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
  Humanoid every frame. Not yet: R15, animations.
- `src/luau/` runs Scripts/LocalScripts/ModuleScripts with mlua (Luau). `ScriptHost` shares the DOM
  (`Rc<RefCell<WeakDom>>`); never hold a DOM borrow while calling into Lua. Waiting is done in
  `prelude.luau` by yielding a marker that the scheduler catches (Rust functions can't yield).
- Frame order (`App::tick`): physics + character write their CFrames into the DOM → scripts step →
  if scripts changed the DOM, rebuild scene + physics (velocities kept, character re-attached, a
  moved HumanoidRootPart = teleport) → character → physics. Physics::new does a zero-length step
  so raycasts work right away. Rebuilding every frame is fine for small places; optimize later.
- Events: signals/connections are Luau tables in `prelude.luau`; `luau/events.rs` says which members
  are events and fires them (only with the DOM unborrowed). Touched comes from `Physics::touches`
  (touching pairs diffed each frame, carried across rebuilds); the character touches through a
  sensor box reported as its lowest part. Death (Health 0 or falling) breaks joints, respawn after
  Players.RespawnTime runs StarterCharacterScripts again and fires CharacterAdded.
- Test places: `examples/scripts.rbxl` (`scripts_demo` example; `app::tests` run the full loop),
  `examples/demo.rbxl` (Kahua's `demo_place` example), `examples/physics.rbxl`
  (`cargo run --example physics_demo -- examples/physics.rbxl`; `cargo test` simulates it).
- Usage limits are tight: one feature per session, keep changes small.

## Plan
1. Engine (offline): ~~render places~~ → ~~physics~~ → ~~character~~ → ~~Luau core~~ → ~~Luau events~~ → GUI
2. Client: home screen, settings, graphics presets, profiles, stats (mockups exist)
3. Online: Roblox servers, server browser. No cheat features.
