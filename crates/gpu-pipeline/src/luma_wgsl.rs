//! Working-space luminance, in WGSL.

/// `prophoto_luminance`, with its coefficients **substituted from [`color::PROPHOTO_LUMA`]**.
///
/// ⚠⚠ **Substituted rather than transcribed, which is strictly stronger than what
/// [`crate::oklab_wgsl`] does.** That prelude carries its matrices as literals and is guarded
/// *behaviourally*, because parsing digits back out of a shader string breaks on any
/// reformatting. Here the whole constant is three numbers, so it can simply be interpolated —
/// and then the two **cannot** disagree, rather than being checked for agreeing.
///
/// ⚠ That matters more than it looks: `color`'s own doc records **six** copies of a Rec.709 luma
/// triple, every one of which became wrong the day the working space stopped being sRGB. This is
/// the seventh place that constant is needed and the first that cannot go stale.
///
/// ⚠ A function rather than a `const`, because `format!` is not a const operation. It is called
/// once per encode, alongside a shader compile that dwarfs it.
pub(crate) fn luma_wgsl() -> String {
    format!(
        r#"
fn prophoto_luminance(r: f32, g: f32, b: f32) -> f32 {{
    return {}f * r + {}f * g + {}f * b;
}}
"#,
        color::PROPHOTO_LUMA[0],
        color::PROPHOTO_LUMA[1],
        color::PROPHOTO_LUMA[2]
    )
}
