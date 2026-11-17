//! What Rustroom detects for the primary monitor (2026-11-18) — the facts `display_detect` gathers and what it decides
//! from them, with the conversion of a few sRGB colours. A diagnostic for "is the preview converted, and into what".
//!
//!     cargo run --release --example display_probe

fn main() {
    let facts = rustroom_app::display_detect::facts(None);
    println!("the OS manages colour: {}", facts.os_managed);
    println!(
        "assigned profile: {}",
        facts
            .icc
            .as_ref()
            .map_or("none (or Windows' generic sRGB)", |(n, _)| n.as_str())
    );
    for (i, e) in facts.edids.iter().enumerate() {
        match color::edid::parse(e) {
            Some(e) => println!(
                "EDID {i}: {} R{:?} G{:?} B{:?} W{:?} sRGB-default {}",
                e.name.as_deref().unwrap_or("(no name)"),
                e.primaries.red,
                e.primaries.green,
                e.primaries.blue,
                e.primaries.white,
                e.srgb_default
            ),
            None => println!("EDID {i}: unreadable ({} bytes)", e.len()),
        }
    }
    let detected = rustroom_app::display_detect::decide(&facts);
    println!("decided: {} ({detected:?})", detected.caption());
    let d = detected.display();
    for (name, c) in [
        ("red", [1.0, 0.0, 0.0]),
        ("green", [0.0, 1.0, 0.0]),
        ("blue", [0.0, 0.0, 1.0]),
    ] {
        let o = d.convert(c);
        println!(
            "sRGB {name:<5} -> display ({:.3}, {:.3}, {:.3})",
            o[0], o[1], o[2]
        );
    }
}
