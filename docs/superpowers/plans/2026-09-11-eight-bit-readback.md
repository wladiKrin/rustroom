# M8.3g — the 8-bit readback: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ⚠⚠ **Project conventions, not negotiable:** commit messages carry **no `Co-Authored-By`
> trailer**; **never push**; work on the branch `m8.3g-eight-bit-readback`.
>
> ⚠ **Read every gate's exit code from a FILE, never through a pipe**, with session-unique log
> names. Read the passed-COUNT, never the word `ok` — a filter that matches nothing prints `ok`.
>
> ⚠ **Before every commit, read `git diff --stat` and `git diff | grep '^-'`.** A scripted edit
> spliced by a computed index destroyed ten tests in this very branch and `git add -A` committed
> it; it was caught by reading `--stat` afterwards, which is one commit too late.
>
> ⚠ Iterate with `cargo test --release -p gpu-pipeline` — a GPU adapter costs 44–75 s in debug
> against 21 ms in release.

**Goal:** when the accelerated tail ends in a GPU chain and nothing downstream needs the float, the
chain quantizes on the GPU and downloads **RGBA8** — 27 MB instead of 82 MB.

**Architecture:** a final quantize dispatch inside `GpuChain`; `accelerate` hands the tail's last
chain back separately so the caller can choose how to run it; `pack` takes a `Packed` enum.

**Tech stack:** `wgpu` 29 compute, WGSL, the existing `GpuCache`.

**Spec:** `docs/superpowers/specs/2026-09-11-eight-bit-readback-design.md`

---

## File structure

- **Create** `crates/gpu-pipeline/src/quantize.rs` — the WGSL and the one dispatch that encodes it.
  Its own file because it is **not** a `GpuStage`: it replaces no CPU stage, it writes to a
  different buffer, and it runs only in bytes mode. Putting it in `chain.rs` would mix "what a run
  is" with "how one shader works".
- **Modify** `crates/gpu-pipeline/src/chain.rs` — one download body parameterised by output mode,
  plus `run_rgba8` and `last_name`.
- **Modify** `crates/gpu-pipeline/src/accelerate.rs` — return `(Pipeline, Option<GpuChain>)`, add
  `rejoin`.
- **Modify** `crates/gpu-pipeline/src/lib.rs` — `mod quantize;` (alphabetical, after `output_transform`).
- **Modify** `crates/app/src/render_engine.rs` — `Packed`, `pack`'s two arms, the gate.

---

### Task 1: the quantize dispatch, and a chain that can download bytes

**Files:**
- Create: `crates/gpu-pipeline/src/quantize.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`
- Modify: `crates/gpu-pipeline/src/chain.rs`

- [ ] **Step 1: write the failing test** — append inside `chain.rs`'s `mod tests`:

```rust
    /// ⚠⚠ **THE test of this milestone.** The GPU's bytes must be what the CPU would have
    /// quantized — not close to it, identical to it, because both sides are a *defined* rounding
    /// of the same number and a tolerance would hide exactly the tie bug below.
    ///
    /// ⚠⚠ **The fixture is the test.** WGSL's `round` breaks ties to the EVEN whole number while
    /// Rust's `f32::round` breaks them AWAY FROM ZERO, and `v * 255` lands on exact halves
    /// routinely — so a shader using `round` is correct on every ordinary value and off by one
    /// level on every tie. A ramp that never lands on a half cannot see that. This fixture carries
    /// the halves `k/510` for odd `k`, plus a negative, a value above 1, and a NaN.
    #[test]
    fn the_gpu_bytes_are_what_the_cpu_would_have_quantized() {
        let Some(ctx) = ctx() else { return };
        let mut px: Vec<f32> = Vec::new();
        // every exact quantization tie: (2k+1)/510 * 255 == k + 0.5
        for k in 0..255u32 {
            let half = (2 * k + 1) as f32 / 510.0;
            px.extend_from_slice(&[half, half, half]);
        }
        px.extend_from_slice(&[-0.25, 1.5, f32::NAN]);
        px.extend_from_slice(&[0.0, 1.0, 0.5]);
        let w = px.len() / 3;
        let img = ImageBuf::from_vec(w, 1, px);
        // ⚠ An identity chain: `exposure` at 0 EV multiplies by exactly 1.0, so the only thing
        // under test is the quantization. A chain that *changed* the pixels would still pass, but
        // a failure could then be the stage rather than the rounding.
        let chain = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(0.0, ctx.clone()))],
        );
        let want = chain.apply(img.clone()).to_rgba8();
        let got = chain.run_rgba8(&img).expect("the GPU path must be available here");
        assert_eq!(got.len(), want.len(), "byte count");
        let bad: Vec<(usize, u8, u8)> = got
            .iter()
            .zip(&want)
            .enumerate()
            .filter(|(_, (g, w))| g != w)
            .map(|(i, (g, w))| (i, *g, *w))
            .take(6)
            .collect();
        assert!(bad.is_empty(), "GPU vs CPU quantize, first few (index, gpu, cpu): {bad:?}");
    }

    /// ⚠ The chain must know what it ends in — `accelerate` asks, to decide whether the tail's
    /// output is display-referred.
    #[test]
    fn a_chain_reports_the_name_of_its_last_stage() {
        let Some(ctx) = ctx() else { return };
        let chain = GpuChain::new(
            ctx.clone(),
            vec![
                Box::new(GpuExposureStage::new(0.0, ctx.clone())),
                Box::new(crate::GpuOutputTransformStage::new(
                    color::OutputSpace::Srgb,
                    &ctx,
                )),
            ],
        );
        assert_eq!(chain.last_name(), Some("output_transform"));
        assert_eq!(GpuChain::new(ctx.clone(), vec![]).last_name(), None);
    }
```

- [ ] **Step 2: run it and watch it fail**

Run: `cargo test --release -p gpu-pipeline -- the_gpu_bytes a_chain_reports`
Expected: FAIL — `no method named run_rgba8` / `last_name`.

- [ ] **Step 3: write `quantize.rs`**

```rust
//! The last dispatch: display-referred floats to packed RGBA8, without going through the CPU.
//!
//! ⚠⚠ **Not a `GpuStage`, deliberately.** It replaces no CPU stage, it writes to a *different*
//! buffer, and it runs only when the chain is downloading bytes — so giving it that trait would
//! mean a `cpu()` twin that does not exist and a `cpu_cost` the gate must never see.

use crate::{GpuContext, GpuImage};

/// ⚠⚠ **`floor(v * 255.0 + 0.5)`, NEVER `round`.** WGSL's `round` breaks ties to the **even**
/// whole number; Rust's `f32::round`, which `ImageBuf::quantize` uses, breaks them **away from
/// zero**. `v * 255` lands on exact halves routinely, so a transliterated `round` is correct on
/// every ordinary value and off by one level on every tie.
///
/// ⚠ **NaN must reach 0, and `clamp` is not what does it.** On the CPU that happens through the
/// saturating `as u8` cast — `f32::clamp` returns NaN for NaN — so the shader tests for it
/// explicitly. `v != v` is true only for NaN.
///
/// ⚠ `pack4x8unorm` was considered and not used: its rounding is specified as the same
/// `floor(0.5 + 255 * clamp(..))`, but writing the rule out is what lets a reader check it against
/// `quantize` without consulting the WGSL spec, and the NaN guard is needed either way.
const SHADER: &str = r#"
struct Params {
    pixels: u32,
    row_groups: u32,
    pad0: u32,
    pad1: u32,
};
@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

fn q(v: f32) -> u32 {
    let x = select(v, 0.0, v != v);
    return u32(floor(clamp(x, 0.0, 1.0) * 255.0 + 0.5));
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    dst[pixel] = q(src[p]) | (q(src[p + 1u]) << 8u) | (q(src[p + 2u]) << 16u) | 0xff000000u;
}
"#;

/// Record the quantize dispatch, writing packed RGBA8 into `dst`.
///
/// ⚠ `dst` is a separate buffer rather than the image's own. Packing in place would have pixel
/// `i` write index `i` while pixel `j > i/3` still needs to read index `i` — a read/write hazard
/// between invocations of one dispatch, which no barrier inside a dispatch can order.
pub(crate) fn encode(
    ctx: &GpuContext,
    pass: &mut wgpu::ComputePass,
    img: &GpuImage,
    dst: &wgpu::Buffer,
) {
    let device = &ctx.device;
    let (row_groups, col_groups) = crate::dispatch::Dispatch::grid(img.pixels());
    let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let layout_entries = [
        entry(0, wgpu::BufferBindingType::Storage { read_only: true }),
        entry(1, wgpu::BufferBindingType::Uniform),
        entry(2, wgpu::BufferBindingType::Storage { read_only: false }),
    ];
    // ⚠ Through the cache, like every other program: keyed on the shader text, so this one
    // compiles once per process rather than once per render.
    let (layout, pipelines) =
        ctx.cache
            .program(device, "quantize", SHADER, &layout_entries, &["main"]);

    let params: [u32; 4] = [img.pixels(), row_groups, 0, 0];
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("quantize"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    ctx.queue
        .write_buffer(&uniform, 0, bytemuck::cast_slice(&params));

    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("quantize"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: img.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: dst.as_entire_binding(),
            },
        ],
    });
    pass.set_pipeline(&pipelines[0]);
    pass.set_bind_group(0, &bind, &[]);
    pass.dispatch_workgroups(row_groups, col_groups, 1);
}
```

- [ ] **Step 4: declare it** — in `crates/gpu-pipeline/src/lib.rs`, add `mod quantize;` between
  `mod output_transform;` and `mod sharpen;`.

- [ ] **Step 5: parameterise the chain's download**

In `chain.rs`, replace the body of `run` so both modes share one upload and one pass. Add above
`impl GpuChain`:

```rust
/// What the chain downloads.
///
/// ⚠⚠ The two modes share **one** upload, one compute pass and one submit — the whole point is
/// that only the *download* differs, so a second code path would be two things that can disagree
/// about how a chain runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Download {
    /// The resident f32 image: `w * h * 12` bytes.
    Float,
    /// Packed RGBA8, quantized by a final dispatch: `w * h * 4` bytes.
    Rgba8,
}
```

Rename `fn run(&self, image: &ImageBuf) -> Option<Vec<f32>>` to
`fn run_with(&self, image: &ImageBuf, mode: Download) -> Option<Vec<u8>>` and make these edits
inside it, leaving the upload block and the compute pass **exactly** as they are:

```rust
        // ── the quantize dispatch, in bytes mode only ─────────────────────────────────
        // ⚠ Inside the same pass as every stage, so it sees their writes under the in-pass
        // barrier this chain already relies on.
        let packed = match mode {
            Download::Float => None,
            Download::Rgba8 => Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chain-packed"),
                size: (image.width() * image.height() * 4) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })),
        };
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            for s in &self.stages {
                s.encode(ctx, &mut pass, &img, &scratch);
            }
            if let Some(p) = &packed {
                crate::quantize::encode(ctx, &mut pass, &img, p);
            }
        }

        // ── download, once ────────────────────────────────────────────────────────────
        // ⚠⚠ In bytes mode this moves **27 MB instead of 82** at rung 3200. Measured: the
        // download costs 6.33 ms + a 6.87 ms `to_vec` at f32 against 2.59 + 2.19 at RGBA8, so
        // 8.4 ms — a 2.8x saving rather than the 4x the size ratio suggests, because a download
        // carries a **1.28 ms fixed cost** that no size reduction touches.
        let (src, out_bytes) = match &packed {
            Some(p) => (p, (image.width() * image.height() * 4) as u64),
            None => (&img.buffer, bytes),
        };
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chain-readback"),
            size: out_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_buffer_to_buffer(src, 0, &readback, 0, out_bytes);
```

and the map block's final line becomes `let out: Vec<u8> = slice.get_mapped_range().to_vec();`.

Then add, in `impl GpuChain`:

```rust
    /// The name of the stage this chain ends with, if any.
    ///
    /// ⚠ `accelerate` asks so it can decide whether the tail's output is display-referred. Asked
    /// rather than inferred from "`output_transform` is always last", which is true today and is
    /// an assumption rather than a guarantee.
    pub fn last_name(&self) -> Option<&'static str> {
        self.stages.last().map(|s| s.name())
    }

    /// Run the chain and download **RGBA8** rather than f32.
    ///
    /// ⚠⚠ Sound **only** when this chain's output is display-referred `[0, 1]`, which is what
    /// `accelerate` establishes before handing the chain over: it is the tail's last stage and its
    /// own last stage is `output_transform`. A caller that reached past that would quantize
    /// scene-referred values, which do not fit in a byte.
    ///
    /// ⚠ `None` on any GPU failure, like `run`. The caller falls back to [`PipelineStage::apply`],
    /// which retries and then takes the CPU path — one wasted attempt on a path that is rare.
    pub fn run_rgba8(&self, image: &ImageBuf) -> Option<Vec<u8>> {
        if self.stages.is_empty() {
            return None;
        }
        self.run_with(image, Download::Rgba8)
    }
```

and `apply`'s call site becomes:

```rust
        match self.run_with(&image, Download::Float) {
            Some(data) => ImageBuf::from_vec(w, h, bytemuck::cast_slice(&data).to_vec()),
            None => self.cpu_fallback(image),
        }
```

- [ ] **Step 6: run the tests**

Run: `cargo test --release -p gpu-pipeline`
Expected: PASS, **71 passed, 1 ignored** (69 before + the 2 new; the ignored one is the probe).

- [ ] **Step 7: sabotage the rounding, and watch it fail**

Change the shader's `floor(clamp(x, 0.0, 1.0) * 255.0 + 0.5)` to
`round(clamp(x, 0.0, 1.0) * 255.0)`. Re-run.
Expected: `the_gpu_bytes_are_what_the_cpu_would_have_quantized` FAILS, naming tie indices.
⚠ If it passes, the fixture is wrong, not the code — check that the halves reached the shader.
Restore from a `cp` backup taken first. **Never `git checkout -- <file>`** — that has destroyed
uncommitted work in this repo twice.

- [ ] **Step 8: commit**

```bash
git diff --stat
git diff | grep '^-[^-]' | head
git add crates/gpu-pipeline/src/quantize.rs crates/gpu-pipeline/src/lib.rs crates/gpu-pipeline/src/chain.rs
git commit
```

---

### Task 2: `accelerate` hands back the tail's last chain

**Files:**
- Modify: `crates/gpu-pipeline/src/accelerate.rs`
- Modify: `crates/app/src/render_engine.rs` (call site only — `rejoin` immediately, so behaviour is unchanged)

- [ ] **Step 1: write the failing tests** — in `accelerate.rs`'s `mod tests`:

```rust
    /// ⚠⚠ The condition the whole gate rests on, answered where it is provable.
    #[test]
    fn a_tail_ending_in_an_accelerated_output_transform_hands_the_chain_back() {
        let Some(ctx) = ctx() else { return };
        let (rest, chain) = accelerate(hsl_tail(), &hsl_edits(), SCALE, profile(), spec(), ctx);
        let chain = chain.expect("the tail ends in a chain");
        assert_eq!(chain.last_name(), Some("output_transform"));
        assert!(
            !rest.stage_names().contains(&"gpu_chain"),
            "the returned pipeline must NOT still hold the chain"
        );
    }

    /// ⚠ A CPU stage after the chain means the tail's output is not the chain's output.
    #[test]
    fn a_cpu_stage_after_the_chain_means_no_chain_is_handed_back() {
        let Some(ctx) = ctx() else { return };
        let (_, chain) = accelerate(hsl_then_vignette_tail(), &hsl_edits(), SCALE, profile(), spec(), ctx);
        assert!(chain.is_none(), "a trailing CPU stage must keep the chain in the pipeline");
    }

    /// ⚠ A run the cost gate refused is not a chain at all — the byte path must not appear from
    /// a tail that was left entirely on the CPU.
    #[test]
    fn a_tail_the_cost_gate_refused_hands_nothing_back() {
        let Some(ctx) = ctx() else { return };
        let (_, chain) = accelerate(neutral_tail(), &Edits::default(), SCALE, profile(), spec(), ctx);
        assert!(chain.is_none());
    }
```

⚠ `hsl_tail`, `hsl_then_vignette_tail`, `neutral_tail`, `profile`, `spec` and `ctx` follow the
existing helpers in that module — reuse them rather than writing new ones, and if a shape is
missing build it with `pipeline::build_pipeline_split_in_region_with_brush` exactly as the existing
tests do.

- [ ] **Step 2: run and watch them fail**

Run: `cargo test --release -p gpu-pipeline -- hands_the_chain_back hands_nothing_back after_the_chain`
Expected: FAIL — `accelerate` returns one value, not a tuple.

- [ ] **Step 3: change `flush` to return the chain rather than push it**

```rust
/// Turn a pending run into a chain, or put the CPU stages back.
///
/// ⚠⚠ **Returns the chain rather than pushing it**, so the caller can see whether the last thing
/// it built was a chain. Pushing it into a `Vec<Box<dyn PipelineStage>>` would make that question
/// answerable only by downcasting.
fn flush(
    run: &mut Vec<Candidate>,
    out: &mut Vec<Box<dyn PipelineStage>>,
    ctx: &Arc<GpuContext>,
) -> Option<GpuChain> {
    if run.is_empty() {
        return None;
    }
    let taken = std::mem::take(run);
    let costs: Vec<f32> = taken.iter().map(|(g, _)| g.cpu_cost()).collect();
    if worth_a_chain(&costs) {
        Some(GpuChain::new(
            ctx.clone(),
            taken.into_iter().map(|(g, _)| g).collect(),
        ))
    } else {
        // ⚠ In order, so a refused run keeps its position exactly as the CPU stages had it.
        out.extend(taken.into_iter().map(|(_, c)| c));
        None
    }
}
```

- [ ] **Step 4: return the pair, and add `rejoin`**

```rust
pub fn accelerate(
    tail: Pipeline,
    edits: &Edits,
    scale: RenderScale,
    profile: ProfileRendering<'_>,
    spec: RenderSpec,
    ctx: &Arc<GpuContext>,
) -> (Pipeline, Option<GpuChain>) {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    let mut run: Vec<Candidate> = Vec::new();
    for s in tail.into_stages() {
        match REPLACEABLE.iter().find(|(n, _)| *n == s.name()) {
            Some((_, build)) => run.push((build(edits, scale, profile, spec, ctx), s)),
            None => {
                // ⚠ A chain followed by a CPU stage is not the tail's last stage, so it goes
                // straight back into the pipeline and is never a candidate for the byte path.
                if let Some(c) = flush(&mut run, &mut out, ctx) {
                    out.push(Box::new(c));
                }
                out.push(s);
            }
        }
    }
    // ⚠⚠ Whatever this produces **is** last, by construction — there is nothing after it to push.
    let last = flush(&mut run, &mut out, ctx);
    match last {
        // ⚠ The name is checked rather than assumed. `output_transform` ends every tail today,
        // which is a fact about the current pipeline and not a guarantee about the next one.
        Some(c) if c.last_name() == Some("output_transform") => (Pipeline::new(out), Some(c)),
        Some(c) => {
            out.push(Box::new(c));
            (Pipeline::new(out), None)
        }
        None => (Pipeline::new(out), None),
    }
}

/// Put a chain back on the end of a pipeline.
///
/// ⚠ For the callers that want one plain `Pipeline` — the soft-proof tail, and every case the
/// gate turns down. Written once so "the pipeline plus its chain" cannot be assembled two ways.
pub fn rejoin(p: Pipeline, chain: Option<GpuChain>) -> Pipeline {
    match chain {
        None => p,
        Some(c) => {
            let mut v = p.into_stages();
            v.push(Box::new(c));
            Pipeline::new(v)
        }
    }
}
```

Export `rejoin` from `lib.rs`: `pub use accelerate::{accelerate, rejoin};`

- [ ] **Step 5: keep the app compiling, unchanged in behaviour**

In `render_engine.rs`, `build_tail`'s accelerate arm becomes:

```rust
                Some(ctx) => {
                    let (p, c) = gpu_pipeline::accelerate(tail, edits, scale, profile, render_spec, ctx);
                    gpu_pipeline::rejoin(p, c)
                }
```

⚠ This task changes **no behaviour at all** — that is deliberate, so a failure in Task 4 cannot be
confused with a failure here.

- [ ] **Step 6: run the gate**

```bash
S=t2-$(date +%s)
cargo fmt --all > /tmp/rr-fmt-$S.log 2>&1; echo "FMT=$?" > /tmp/rr-gate-$S.log
cargo clippy --all-targets --all-features -- -D warnings > /tmp/rr-clippy-$S.log 2>&1; echo "CLIPPY=$?" >> /tmp/rr-gate-$S.log
cargo test --workspace > /tmp/rr-test-$S.log 2>&1; echo "TEST=$?" >> /tmp/rr-gate-$S.log
cat /tmp/rr-gate-$S.log
```
Expected: all three `0`.

- [ ] **Step 7: sabotage, and watch it fail**

Drop the `c.last_name() == Some("output_transform")` guard so any final chain is handed back.
Expected: no existing test fails — ⚠ **record that honestly rather than inventing a test that
appears to cover it**: the guard is defence against a *future* tail whose last stage is not
`output_transform`, and no tail today can reach that state. Restore from the `cp` backup.

- [ ] **Step 8: commit**

---

### Task 3: `pack` takes a `Packed`

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: write the failing test**

```rust
    /// ⚠ The bytes must reach the texture untouched, and the histogram must come from them —
    /// which it already does on the float path, so the two arms agree by construction rather
    /// than by a second implementation.
    #[test]
    fn packing_bytes_uses_them_for_both_the_texture_and_the_histogram() {
        let buf = ImageBuf::from_vec(2, 1, vec![0.0, 0.5, 1.0, 1.0, 0.5, 0.0]);
        let want = pack(Packed::Float(buf.clone()), None, None, 16, color::OutputSpace::Srgb, None);
        let got = pack(
            Packed::Rgba8 { bytes: buf.to_rgba8(), w: 2, h: 1 },
            None,
            None,
            16,
            color::OutputSpace::Srgb,
            None,
        );
        assert_eq!(*got.rgba, *want.rgba, "texture");
        assert_eq!(got.hist, want.hist, "histogram");
        assert_eq!((got.width, got.height), (2, 1));
        assert!(
            Arc::ptr_eq(&got.rgba, &got.srgb),
            "the two must be one allocation, as on the float sRGB path"
        );
    }
```

- [ ] **Step 2: run and watch it fail** — `cargo test -p app --lib packing_bytes`, expected FAIL
  (`Packed` does not exist).

- [ ] **Step 3: add the type and split `pack`'s head**

```rust
/// What a finished render hands to `pack`.
///
/// ⚠⚠ The `Rgba8` arm carries **no `ImageBuf`**, and that is what keeps it small: the histogram
/// already comes from the quantized bytes, and `srgb` and `rgba` become one shared allocation.
/// ⚠ It is constructible only where the gate held — a non-sRGB display space, Visualize Spots and
/// soft proofing all need the float, and on this arm they are unrepresentable rather than guarded.
pub enum Packed {
    Float(ImageBuf),
    Rgba8 { bytes: Vec<u8>, w: usize, h: usize },
}
```

`pack`'s first parameter becomes `packed: Packed`, and its body begins:

```rust
    let (out, width, height, pre) = match packed {
        Packed::Float(out) => {
            let (w, h) = (out.width(), out.height());
            (Some(out), w, h, None)
        }
        Packed::Rgba8 { bytes, w, h } => (None, w, h, Some(bytes)),
    };
    // ⚠ Quantized to sRGB *first*, and the histogram taken from that — unchanged, except that on
    // the byte path the GPU has already done the quantizing.
    let srgb8 = match pre {
        Some(b) => b,
        None => out.as_ref().expect("float arm").to_rgba8(),
    };
```

⚠ The `visualize` and `display_space != Srgb` arms already need `out`; on the byte path they are
unreachable, so they take `out.as_ref().expect("the gate keeps the float path for this")`. The
`expect` message is the documentation of the invariant.

- [ ] **Step 4: update the four existing `pack(` call sites** in `render_engine.rs` (two production,
  four in tests) to wrap their buffer in `Packed::Float(..)`.

- [ ] **Step 5: run the gate** (as Task 2 Step 6). Expected: all `0`, test count unchanged +1.

- [ ] **Step 6: commit**

---

### Task 4: the gate, and the bytes reaching the texture

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: write the failing tests**

```rust
    /// ⚠⚠ THE CARRIER. Every test above can pass while nothing reaches the texture — which is how
    /// creative LUTs shipped fully tested and 100% dead.
    #[test]
    fn a_frame_rendered_through_the_bytes_is_the_frame_rendered_through_the_float() { /* … */ }

    /// ⚠ One per condition, each asserting the FLOAT path is taken.
    #[test]
    fn visualize_spots_keeps_the_float_path() { /* … */ }
    #[test]
    fn a_soft_proof_keeps_the_float_path() { /* … */ }
    #[test]
    fn a_non_srgb_display_space_keeps_the_float_path() { /* … */ }
    /// ⚠⚠ And the fourth, without which the three above pass against an implementation that
    /// never takes the byte path at all.
    #[test]
    fn with_none_of_them_active_the_bytes_are_used() { /* … */ }
```

⚠ The four gate tests assert on a **counter** of byte-path renders on `RenderEngine`, in the shape
`requests_issued` already takes — unconditional, not `#[cfg(test)]`, because `render_engine` is in
the lib and a gated field would not exist where a bin test needs it.

- [ ] **Step 2: run and watch them fail.**

- [ ] **Step 3: derive the gate once**

In `render_cached`, above `build_tail`:

```rust
    // ⚠⚠ **Derived ONCE and passed down, never re-derived.** Three things downstream need the
    // float: a non-sRGB display space (`pack` converts from it deliberately — an 8-bit → 8-bit
    // conversion costs ~0.8 bits and bands a smooth sky), Visualize Spots (the edge view is
    // computed from it) and soft proofing (the ProPhoto run is decoded back to the working space).
    // ⚠ The grade range view is a fourth, and needs nothing: it bypasses `accelerate` entirely.
    let float_needed = display_space != color::OutputSpace::Srgb
        || visualize.is_some()
        || soft_proof.is_some();
```

and the plain arm of the `grade_view` match becomes:

```rust
        None => {
            let (p, chain) = build_tail(color::OutputSpace::Srgb);
            match chain.filter(|_| !float_needed) {
                Some(c) => {
                    let mid = p.run(input);
                    let (w, h) = (mid.width(), mid.height());
                    match c.run_rgba8(&mid) {
                        Some(bytes) => (Packed::Rgba8 { bytes, w, h }, None),
                        // ⚠ A GPU failure falls back through `apply`, which retries once and then
                        // takes the CPU path. Rare, and one wasted attempt beats a second
                        // fallback implementation here.
                        None => (Packed::Float(c.apply(mid)), None),
                    }
                }
                None => (
                    Packed::Float(gpu_pipeline::rejoin(p, chain).run(input)),
                    None,
                ),
            }
        }
```

⚠ `build_tail` now returns the pair; the proof arm calls `rejoin` immediately.

- [ ] **Step 4: run the tests, then the full gate.**

- [ ] **Step 5: sabotage each condition** — force `float_needed = false` and watch the three
  condition tests fail; force it `true` and watch `with_none_of_them_active_the_bytes_are_used`
  fail. Restore from `cp` backups.

- [ ] **Step 6: commit**

---

### Task 5: measure, look at it, gate, record

- [ ] **Step 1: measure the real engine**

```bash
cargo run --release -p app --example bench_gpu_tail -- ~/Pictures/fuji/DSCF6291.RAF
```
⚠ On a quiet machine — this project has twice quoted a contaminated bench. Record the TYPICAL row
at rung 3200 against M8.3f's **42.32 ms / 1.88x**, and say plainly if it misses the ~31.7 ms
projection rather than quoting the projection as a result.

- [ ] **Step 2: re-run `probe_breakdown`** and record what the download now costs, so the claim is
  measured on both sides rather than inferred from the earlier table.

- [ ] **Step 3: launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
nohup cargo run --release -p app > /tmp/rustroom-gui.log 2>&1 &
```
What to look at, and it is specific: **the picture must not change.** ⚠ A quantization bug here is
a **uniform one-level shift** — invisible on a photograph and visible in the histogram, so compare
the histogram with `gpu_tail` on and off. ⚠ Then turn **Visualize Spots** on and off, and a **soft
proof** on and off: those switch the float path back, and a broken gate shows there first.

- [ ] **Step 4: the full gate**, exit codes from files, and confirm **63 goldens ran and none
  moved** and **0 GPU tests skipped**.

- [ ] **Step 5: record it in `CLAUDE.md`** — a new M8.3g section before M8.3f's, the RESUME block's
  *State at handoff* and *NEXT ACTION* rewritten. ⚠ The numbering note: the mask loop becomes
  **M8.3h**. ⚠ Include the process failure recorded on this branch — a scripted edit spliced by a
  computed index deleted ten tests and was committed — because it is the second instance and the
  rule that catches it already existed.

- [ ] **Step 6: commit, and stop.** Do **not** merge and do **not** push.
