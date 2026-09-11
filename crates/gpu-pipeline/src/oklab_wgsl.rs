//! The Oklab conversions, in WGSL, shared by every shader that needs them.

/// `cbrt_signed`, ProPhoto→Oklab and Oklab→ProPhoto, as WGSL source.
///
/// ⚠⚠ **One copy, because a second is how the two come to disagree.** This project already
/// records **six** copies of a Rec.709 luma triple, every one of which became wrong the day the
/// working space stopped being sRGB — and WGSL has no compiler checking these against
/// `color::oklab`. The guard is behavioural rather than textual: with a neutral grade the GPU
/// grading stage reduces to exactly this round trip, so
/// `the_wgsl_prelude_agrees_with_color_oklab` compares it against the Rust it must match.
///
/// ⚠⚠ **`sign(x) * pow(abs(x), 1/3)`, never `pow(x, 1/3)`.** WGSL's `pow` returns NaN for a
/// negative base, and the CPU uses `cbrt` **specifically because it is sign-preserving** — a wide
/// working space produces scene-referred negatives routinely, and `color::oklab` says so at the
/// call site. A naive port turns every such pixel into NaN, which then propagates through the
/// output matrix and paints holes.
///
/// ⚠ Crate-internal rather than public API — the narrow version of the `pub use stages::hue_table`
/// decision, which deliberately re-exported one item instead of opening a whole module.
pub(crate) const OKLAB_WGSL: &str = r#"
fn cbrt_signed(x: f32) -> f32 { return sign(x) * pow(abs(x), 0.3333333333); }

fn prophoto_to_oklab(r: f32, g: f32, bl: f32) -> vec3<f32> {
    let lc = 0.7153451038*r + 0.3527967327*g - 0.0682510308*bl;
    let mc = 0.2744153967*r + 0.6678313229*g + 0.0577801949*bl;
    let sc = 0.1098335093*r + 0.1862950046*g + 0.7043511321*bl;
    let l_ = cbrt_signed(lc); let m_ = cbrt_signed(mc); let s_ = cbrt_signed(sc);
    return vec3<f32>(
        0.2104542553*l_ + 0.7936177850*m_ - 0.0040720468*s_,
        1.9779984951*l_ - 2.4285922050*m_ + 0.4505937099*s_,
        0.0259040371*l_ + 0.7827717662*m_ - 0.8086757660*s_
    );
}

fn oklab_to_prophoto(bigL: f32, A: f32, B: f32) -> vec3<f32> {
    let li = bigL + 0.3963377774*A + 0.2158037573*B;
    let mi = bigL - 0.1055613458*A - 0.0638541728*B;
    let si = bigL - 0.0894841775*A - 1.2914855480*B;
    let l3 = li*li*li; let m3 = mi*mi*mi; let s3 = si*si*si;
    return vec3<f32>(
        1.7386588588*l3 - 0.9880931444*m3 + 0.2495310446*s3,
        -0.7071491793*l3 + 1.9343303974*m3 - 0.2272015198*s3,
        -0.0840839768*l3 - 0.3575352425*m3 + 1.4409285251*s3
    );
}
"#;
