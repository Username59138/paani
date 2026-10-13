//! Writes a place whose scripts change the world: `cargo run --example scripts_demo -- examples/scripts.rbxl`.

use kahua_io::rbx_dom_weak::types::{Color3, Ref, Vector3};
use kahua_io::rbx_dom_weak::{InstanceBuilder, WeakDom};
use kahua_io::template;

/// Raining balls, from a Script in ServerScriptService.
const BALL_RAIN: &str = r#"
local colors = {
    Color3.fromRGB(255, 89, 89),
    Color3.fromRGB(255, 200, 60),
    Color3.fromRGB(90, 200, 120),
    Color3.fromRGB(80, 160, 255),
}
for i = 1, 20 do
    local ball = Instance.new("Part")
    ball.Name = "Ball" .. i
    ball.Shape = Enum.PartType.Ball
    ball.Size = Vector3.new(3, 3, 3)
    ball.Color = colors[(i - 1) % #colors + 1]
    ball.Position = Vector3.new(math.random(-6, 6), 40, -20 + math.random(-6, 6))
    ball.Parent = workspace
    wait(0.5)
end
print("All balls dropped")
"#;

/// Turns its part a little every frame.
const SPINNER: &str = r#"
local part = script.Parent
while true do
    part.CFrame = part.CFrame * CFrame.Angles(0, math.rad(2), 0)
    task.wait()
end
"#;

/// Cycles its part through the rainbow.
const RAINBOW: &str = r#"
local part = script.Parent
local hue = 0
while true do
    hue = (hue + 0.01) % 1
    part.Color = Color3.fromHSV(hue, 0.8, 1)
    task.wait()
end
"#;

/// A LocalScript in StarterCharacterScripts: runs inside the character.
const SPEED: &str = r#"
local humanoid = script.Parent:WaitForChild("Humanoid")
humanoid.WalkSpeed = 24
print("Hello, " .. game:GetService("Players").LocalPlayer.Name .. "! WalkSpeed is now " .. humanoid.WalkSpeed)
"#;

/// The classic kill brick.
const KILL_BRICK: &str = r#"
script.Parent.Touched:Connect(function(hit)
    local humanoid = hit.Parent:FindFirstChildOfClass("Humanoid")
    if humanoid then
        humanoid.Health = 0
    end
end)
"#;

/// A coin that disappears when the character touches it.
const COIN: &str = r#"
local coin = script.Parent
coin.Touched:Connect(function(hit)
    if hit.Parent:FindFirstChildOfClass("Humanoid") then
        print("Coin collected!")
        coin:Destroy()
    end
end)
"#;

/// Reports joins, deaths and respawns.
const PLAYERS: &str = r#"
game:GetService("Players").PlayerAdded:Connect(function(player)
    print(player.Name .. " joined")
    player.CharacterAdded:Connect(function(character)
        print("Character spawned")
        character:WaitForChild("Humanoid").Died:Connect(function()
            print(player.Name .. " died")
        end)
    end)
end)
"#;

fn script(class: &str, name: &str, source: &str) -> InstanceBuilder {
    InstanceBuilder::new(class).with_name(name).with_property("Source", source)
}

fn child(dom: &WeakDom, parent: Ref, name: &str) -> Ref {
    let parent = dom.get_by_ref(parent).unwrap();
    parent.children().iter().copied().find(|&r| dom.get_by_ref(r).unwrap().name == name).unwrap()
}

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "scripts.rbxl".into());
    let mut dom = template::baseplate();
    let ws = template::workspace(&dom);
    let root = dom.root_ref();

    dom.insert(child(&dom, root, "ServerScriptService"), script("Script", "BallRain", BALL_RAIN));

    let spinner = dom.insert(
        ws,
        template::part(
            "Spinner",
            "Part",
            Vector3::new(20.0, 3.0, -20.0),
            Vector3::new(14.0, 1.0, 2.0),
            Color3::new(0.9, 0.5, 0.2),
        ),
    );
    dom.insert(spinner, script("Script", "Spin", SPINNER));

    let rainbow = dom.insert(
        ws,
        template::part(
            "Rainbow",
            "Part",
            Vector3::new(-20.0, 3.0, -20.0),
            Vector3::new(6.0, 6.0, 6.0),
            Color3::new(1.0, 1.0, 1.0),
        ),
    );
    dom.insert(rainbow, script("Script", "Colors", RAINBOW));

    let kill_brick = dom.insert(
        ws,
        template::part(
            "KillBrick",
            "Part",
            Vector3::new(12.0, 0.5, 0.0),
            Vector3::new(4.0, 1.0, 8.0),
            Color3::new(1.0, 0.1, 0.1),
        ),
    );
    dom.insert(kill_brick, script("Script", "Kill", KILL_BRICK));

    let coin = dom.insert(
        ws,
        template::part(
            "Coin",
            "Part",
            Vector3::new(-12.0, 2.0, 0.0),
            Vector3::new(0.4, 3.0, 3.0),
            Color3::new(1.0, 0.8, 0.1),
        )
        .with_property("Shape", kahua_io::rbx_dom_weak::types::Enum::from_u32(2))
        .with_property("CanCollide", false),
    );
    dom.insert(coin, script("Script", "Collect", COIN));

    dom.insert(child(&dom, root, "ServerScriptService"), script("Script", "Players", PLAYERS));

    let starter_player = child(&dom, root, "StarterPlayer");
    dom.insert(child(&dom, starter_player, "StarterCharacterScripts"), script("LocalScript", "Speed", SPEED));

    kahua_io::write_file(&dom, std::path::Path::new(&out))?;
    println!("wrote {out}");
    Ok(())
}
