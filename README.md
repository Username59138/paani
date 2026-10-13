# Paani

An open-source client for Roblox places, written in Rust. Companion to
[Kahua](https://github.com/Username59138/kahua): build in Kahua, play in Paani.

> Early work in progress. Paani is not affiliated with Roblox Corporation.

## Run

```sh
git clone https://github.com/Username59138/paani
cd paani
cargo run --release -- examples/demo.rbxl
cargo run --release -- examples/physics.rbxl   # things that fall
```

## Controls

| Input | Action |
|---|---|
| W A S D | Walk |
| Space | Jump |
| Right mouse (hold) | Turn the camera |
| Scroll | Zoom |
| F | Free camera (WASD, Q/E down/up, Shift faster) |
| R | Restart the physics |
| Esc | Release the mouse |

## Roadmap

1. **Engine** (offline): render places ✅, physics ✅, character ✅, Luau scripts
2. **Client**: home screen, settings, graphics presets, profiles, stats
3. **Online**: join Roblox servers, server browser

## License

GPL-3.0-or-later
