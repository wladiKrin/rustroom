//! M10.13a: Rustroom's DCP maths against Adobe's DNG SDK, the reference implementation, at the function level.
//!
//! `cargo run --release -p raw-decode --example dcp_oracle <profile.dcp> <oracle.txt>`
//!
//! `oracle.txt` is the SDK's answer for a list of camera neutrals — written by a small C++ harness linked against the
//! SDK's own `dng_color_spec` and `RefBaselineHueSatMap` (not in this repo: the SDK is Adobe's). Per neutral it holds
//! `N n0 n1 n2 temperature fm00..fm22` — the forward matrix on white-balanced camera RGB, rows scaled so Y's row sums
//! to 1 — and `H i r g b r' g' b'`, the blended HueSatMap applied to a fixed grid of linear ProPhoto colours. This
//! prints, per neutral, how far our temperature, matrix and table answer sit from it.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let dcp = PathBuf::from(args.next().expect("a .dcp"));
    let oracle = std::fs::read_to_string(args.next().expect("the SDK's oracle.txt"))
        .expect("read the oracle");
    let p = camera_profile::CameraProfile::from_file(&dcp).expect("parse the profile");
    let (f1, f2) = (
        p.forward1.expect("ForwardMatrix1"),
        p.forward2.expect("ForwardMatrix2"),
    );
    let t1 = color::camera::illuminant_temperature(p.illuminant1);
    let t2 = color::camera::illuminant_temperature(p.illuminant2);
    println!(
        "illuminants {} / {} → ours {t1} K / {t2} K",
        p.illuminant1, p.illuminant2
    );
    if std::env::var_os("DCP_ORACLE_DUMP").is_some() {
        println!(
            "color1 {:?}\ncolor2 {:?}\nforward1 {f1:?}\nforward2 {f2:?}",
            p.color1, p.color2
        );
    }

    let (mut worst_t, mut worst_m, mut worst_h, mut worst_a) = (0f64, 0f32, 0f32, 0f32);
    let mut rows: Vec<(f64, f64, f32, f32)> = Vec::new();
    let mut lines = oracle.lines().peekable();
    while let Some(l) = lines.next() {
        let v: Vec<&str> = l.split_whitespace().collect();
        if v.first() != Some(&"N") {
            continue;
        }
        let n = [v[1], v[2], v[3]].map(|s| s.parse::<f32>().unwrap());
        let sdk_t: f64 = v[4].parse().unwrap();
        let sdk_m: Vec<f32> = v[5..14].iter().map(|s| s.parse().unwrap()).collect();
        let ours_t = color::camera::neutral_temperature(n, p.color1, p.color2, t1, t2);
        let g = color::camera::illuminant_weight(ours_t, t1, t2);
        let fm = color::camera::blend(
            color::camera::normalize_forward_matrix(f1),
            color::camera::normalize_forward_matrix(f2),
            g,
        );
        // Ours, scaled the oracle's way: Y's row sums to 1.
        let s: f32 = fm[1].iter().sum();
        let dm = (0..9)
            .map(|k| (fm[k / 3][k % 3] / s - sdk_m[k]).abs())
            .fold(0f32, f32::max);
        // The table, blended on our weight, against the SDK's on its own.
        let table = p.hue_sat_map.as_ref().map(|m| {
            let a = color::huesatmap::Table {
                dims: m.dims,
                data: m.data1.clone(),
            };
            match &m.data2 {
                Some(d2) => color::huesatmap::Table::blend(
                    &a,
                    &color::huesatmap::Table {
                        dims: m.dims,
                        data: d2.clone(),
                    },
                    g,
                ),
                None => a,
            }
        });
        // The same table at the SDK's OWN weight — its temperature, its illuminant temperatures (2850 K, 5000 K, …) —
        // and its `Pin` of the value at 1, so the table's application is compared alone.
        let sdk_temps = |code: u16| match code {
            17 | 3 => 2850.0,
            23 => 5000.0,
            21 | 19 => 6500.0,
            20 | 1 | 18 => 5500.0,
            22 => 7500.0,
            _ => 5000.0,
        };
        let sdk_g = color::camera::illuminant_weight(
            sdk_t as f32,
            sdk_temps(p.illuminant1),
            sdk_temps(p.illuminant2),
        );
        let alone = p.hue_sat_map.as_ref().map(|m| {
            let a = color::huesatmap::Table {
                dims: m.dims,
                data: m.data1.clone(),
            };
            match &m.data2 {
                Some(d2) => color::huesatmap::Table::blend(
                    &a,
                    &color::huesatmap::Table {
                        dims: m.dims,
                        data: d2.clone(),
                    },
                    sdk_g,
                ),
                None => a,
            }
        });
        let mut da = 0f32;
        let mut dh = 0f32;
        while let Some(h) = lines.peek() {
            let w: Vec<&str> = h.split_whitespace().collect();
            if w.first() != Some(&"H") {
                break;
            }
            let f: Vec<f32> = w[2..8].iter().map(|s| s.parse().unwrap()).collect();
            if let Some(t) = &table {
                let got = t.apply([f[0], f[1], f[2]]);
                for c in 0..3 {
                    assert!(got[c].is_finite(), "non-finite at {f:?}");
                    let d = (got[c] - f[3 + c]).abs();
                    if d > dh && std::env::var_os("DCP_ORACLE_WORST").is_some() {
                        eprintln!("  {:?} → ours {got:?}, SDK {:?}", &f[..3], &f[3..]);
                    }
                    dh = dh.max(d);
                }
            }
            if let Some(t) = &alone {
                let [h, s, v] = color::huesatmap::rgb_to_hsv([f[0], f[1], f[2]]);
                let _ = (h, s);
                let got = t.apply([f[0], f[1], f[2]]);
                // The SDK pins the scaled value at 1 (2.5-D table, no over-range); ours keeps a highlight's headroom.
                let mx = got.iter().cloned().fold(0f32, f32::max);
                let got = if mx > 1.0 { got.map(|c| c / mx) } else { got };
                let _ = v;
                for c in 0..3 {
                    da = da.max((got[c] - f[3 + c]).abs());
                }
            }
            lines.next();
        }
        worst_t = worst_t.max((f64::from(ours_t) - sdk_t).abs() / sdk_t);
        worst_m = worst_m.max(dm);
        worst_h = worst_h.max(dh);
        worst_a = worst_a.max(da);
        rows.push((sdk_t, f64::from(ours_t), dm, dh));
    }
    assert!(!rows.is_empty(), "no white balances in the oracle");
    println!(
        "{:>10} {:>10} {:>10} {:>10}",
        "SDK K", "ours K", "matrix Δ", "table Δ"
    );
    for (s, o, m, h) in &rows {
        println!("{s:>10.0} {o:>10.0} {m:>10.5} {h:>10.5}");
    }
    println!(
        "{} white balances: temperature worst {:.2} %, matrix worst {worst_m:.5}, table worst {worst_h:.5}",
        rows.len(),
        worst_t * 100.0
    );
    println!("the table alone, at the SDK's weight and with its value pin: worst {worst_a:.6}");
}
