//! Writes a place for trying out physics: `cargo run --example physics_demo -- examples/physics.rbxl`.

use kahua_io::rbx_dom_weak::InstanceBuilder;
use kahua_io::rbx_dom_weak::types::{Color3, Enum, Vector3};
use kahua_io::template;
use kahua_render::glam::{Quat, Vec3};
use kahua_render::scene::cframe_from_pos_rot;

fn part(name: &str, class: &str, pos: Vec3, size: Vec3, color: (f32, f32, f32), anchored: bool) -> InstanceBuilder {
    template::part(
        name,
        class,
        Vector3::new(pos.x, pos.y, pos.z),
        Vector3::new(size.x, size.y, size.z),
        Color3::new(color.0, color.1, color.2),
    )
    .with_property("Anchored", anchored)
}

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "physics.rbxl".into());
    let mut dom = template::baseplate();
    let ws = template::workspace(&dom);

    // A tower of loose blocks, slightly offset so it wobbles.
    let tower = dom.insert(ws, InstanceBuilder::new("Model").with_name("Tower"));
    for i in 0..10 {
        let t = i as f32 / 9.0;
        let x = if i % 2 == 0 { 0.0 } else { 0.4 };
        dom.insert(
            tower,
            part(
                &format!("Block{i}"),
                "Part",
                Vec3::new(x, 1.0 + i as f32 * 2.0, 0.0),
                Vec3::new(4.0, 2.0, 4.0),
                (0.95, 0.4 + 0.4 * t, 0.2),
                false,
            ),
        );
    }

    // An anchored ramp with balls and a cylinder rolling down it.
    let ramp = dom.insert(ws, InstanceBuilder::new("Model").with_name("Ramp"));
    dom.insert(
        ramp,
        part("Slope", "WedgePart", Vec3::new(30.0, 5.0, 0.0), Vec3::new(14.0, 10.0, 24.0), (0.5, 0.5, 0.55), true),
    );
    for (i, color) in [(0.2, 0.6, 1.0), (1.0, 0.3, 0.5), (0.3, 0.9, 0.4)].into_iter().enumerate() {
        dom.insert(
            ramp,
            part(
                &format!("Ball{i}"),
                "Part",
                Vec3::new(26.0 + i as f32 * 4.0, 14.0 + i as f32 * 3.0, 9.0),
                Vec3::splat(3.0),
                color,
                false,
            )
            .with_property("Shape", Enum::from_u32(0)),
        );
    }
    dom.insert(
        ramp,
        part("Barrel", "Part", Vec3::new(30.0, 13.0, 4.0), Vec3::new(6.0, 3.0, 3.0), (0.6, 0.35, 0.15), false)
            .with_property("Shape", Enum::from_u32(2)),
    );

    // Two parts joined by a WeldConstraint fall as one piece.
    let hammer = dom.insert(ws, InstanceBuilder::new("Model").with_name("Hammer"));
    let handle = dom.insert(
        hammer,
        part("Handle", "Part", Vec3::new(-20.0, 30.0, 0.0), Vec3::new(1.0, 10.0, 1.0), (0.55, 0.35, 0.2), false)
            .with_property("CFrame", cframe_from_pos_rot(Vec3::new(-20.0, 30.0, 0.0), Quat::from_rotation_z(0.5))),
    );
    let head = dom.insert(
        hammer,
        part("Head", "Part", Vec3::new(-22.4, 34.4, 0.0), Vec3::new(4.0, 2.0, 2.0), (0.4, 0.4, 0.45), false)
            .with_property("CFrame", cframe_from_pos_rot(Vec3::new(-22.4, 34.4, 0.0), Quat::from_rotation_z(0.5))),
    );
    dom.insert(
        hammer,
        InstanceBuilder::new("WeldConstraint")
            .with_name("WeldConstraint")
            .with_property("Part0", handle)
            .with_property("Part1", head),
    );

    // A block lands on an invisible anchored wall and seems to float, like in Roblox.
    dom.insert(
        ws,
        part("InvisibleWall", "Part", Vec3::new(-10.0, 5.0, 12.0), Vec3::new(8.0, 10.0, 8.0), (1.0, 1.0, 1.0), true)
            .with_property("Transparency", 1.0f32),
    );
    dom.insert(
        ws,
        part("Floater", "Part", Vec3::new(-10.0, 20.0, 12.0), Vec3::new(3.0, 3.0, 3.0), (1.0, 0.85, 0.1), false),
    );

    // Move the template's spawn out from under the tower.
    let spawn = dom.descendants_of(ws).find(|i| i.class == "SpawnLocation").unwrap().referent();
    dom.get_by_ref_mut(spawn)
        .unwrap()
        .properties
        .insert("CFrame".into(), cframe_from_pos_rot(Vec3::new(0.0, 0.5, -30.0), Quat::IDENTITY).into());

    kahua_io::write_file(&dom, std::path::Path::new(&out))?;
    println!("wrote {out}");
    Ok(())
}
