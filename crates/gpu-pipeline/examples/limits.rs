//! What this machine's GPU will actually accept, against what each render rung needs.
//!
//! ⚠⚠ **Kept because a device limit is the one GPU failure that is invisible until it is fatal.**
//! Under `wgpu::Limits::default()` the ceiling here is **128 MiB** — the storage-binding limit,
//! tighter than the 256 MiB buffer limit — and a full-resolution 26 Mpx frame needs 298 MiB. Every
//! whole-image render above ~11 Mpx therefore failed `create_buffer` validation, which `wgpu`
//! reports by **panicking on the calling thread**: the render worker died and the app stopped
//! updating for the rest of the session. `GpuContext::new` asks the adapter for its real limits
//! now, and `GpuImage::new` declines rather than allocating past them.
//!
//! Run with `cargo run --release -p gpu-pipeline --example limits`.
fn main() {
    let Some(ctx) = gpu_pipeline::shared() else {
        println!("no GPU adapter on this machine — every render takes the CPU path");
        return;
    };
    let budget = ctx.max_image_bytes();
    let mib = |b: u64| b as f64 / 1_048_576.0;
    println!("adapter: {} ({})", ctx.adapter_name(), ctx.backend());
    println!("largest resident image: {:.1} MiB", mib(budget));
    let d = wgpu::Limits::default();
    println!(
        "  (the downlevel default would be {:.1} MiB — the min of a {:.0} MiB buffer and a \
         {:.0} MiB storage binding)",
        mib(d.max_buffer_size.min(d.max_storage_buffer_binding_size)),
        mib(d.max_buffer_size),
        mib(d.max_storage_buffer_binding_size),
    );
    println!();
    println!("  rung   pixels        image     fits?");
    // The doubling ladder a 6246 x 4170 buffer produces, which is an X-T4 at full resolution.
    for long in [1600usize, 3200, 6246] {
        let (w, h) = (long, long * 4170 / 6246);
        let bytes = (w * h * 3 * 4) as u64;
        println!(
            "  {long:>5}   {:>5.1} Mpx   {:>6.1} MiB   {}",
            (w * h) as f64 / 1e6,
            mib(bytes),
            if bytes <= budget { "yes" } else { "NO" }
        );
    }
}
