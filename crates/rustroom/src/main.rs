use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

mod format_arg;
mod space_arg;

/// What `--quality` means when it is not given. Only JPEG reads it.
const DEFAULT_JPEG_QUALITY: u8 = 92;

#[derive(Parser)]
#[command(name = "rustroom", about = "Rustroom — open-source RAW photo editor")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decode a RAW file and write it as an 8-bit TIFF (display-referred, quick dump).
    Decode {
        /// Input RAW file.
        input: PathBuf,
        /// Output image (.tiff).
        output: PathBuf,
    },
    /// Decode a RAW, run the develop pipeline, and export it.
    Render {
        /// Input RAW file.
        input: PathBuf,
        /// Output image. Its extension picks the format unless --format says otherwise.
        output: PathBuf,
        /// Optional edits sidecar (JSON). Omitted = no edits.
        #[arg(long)]
        edits: Option<PathBuf>,
        /// Output format: jpeg, png8, png16, tiff8, tiff16. Omitted = inferred from the
        /// output extension, at 8 bits.
        #[arg(long)]
        format: Option<String>,
        /// JPEG quality 1..=100. Ignored by the lossless formats. Default 92.
        #[arg(long)]
        quality: Option<u8>,
        /// Demosaic quality: fastest, fast, better, best. Higher is slower — roughly 1.2 s
        /// to 11 s per photo on a 26 Mpx RAW.
        #[arg(long, default_value = "fast")]
        demosaic: String,
        /// Output colour space: srgb, p3, adobergb, prophoto. Omitted = srgb.
        #[arg(long)]
        space: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Decode { input, output } => decode(&input, &output),
        Command::Render {
            input,
            output,
            edits,
            format,
            quality,
            demosaic,
            space,
        } => render(
            &input,
            &output,
            edits.as_deref(),
            format.as_deref(),
            quality,
            &demosaic,
            space.as_deref(),
        ),
    }
}

fn decode(input: &Path, output: &Path) -> Result<()> {
    let decoded = raw_decode::decode_raw(input, &raw_decode::DecodeOptions::display())
        .with_context(|| format!("decoding {}", input.display()))?;
    let m = &decoded.metadata;
    eprintln!("Decoded {}x{}  {} {}", m.width, m.height, m.make, m.model);

    // f32 [0,1] interleaved RGB -> 8-bit for a viewable TIFF.
    let buf: Vec<u8> = decoded
        .image
        .data()
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();

    image::save_buffer(
        output,
        &buf,
        m.width as u32,
        m.height as u32,
        image::ColorType::Rgb8,
    )
    .with_context(|| format!("writing {}", output.display()))?;

    eprintln!("Wrote {}", output.display());
    Ok(())
}

fn render(
    input: &Path,
    output: &Path,
    edits_path: Option<&Path>,
    format: Option<&str>,
    quality: Option<u8>,
    demosaic: &str,
    space: Option<&str>,
) -> Result<()> {
    // Resolved before the decode, so a bad --format fails in milliseconds rather than after
    // a multi-second decode and render.
    let format = format_arg::resolve(output, format)?;
    let space = match space {
        Some(s) => space_arg::parse(s)?,
        None => color::OutputSpace::Srgb,
    };
    // ⚠ Warn, never block. Banding in 8-bit is a real hazard for ProPhoto and ordinary practice
    // for the other spaces — see `OutputSpace::warns_in_8_bit`.
    if space.warns_in_8_bit()
        && !matches!(
            format,
            export::ExportFormat::Png16 | export::ExportFormat::Tiff16
        )
    {
        eprintln!(
            "warning: {} in 8-bit will band on smooth gradients — use png16 or tiff16",
            space.label()
        );
    }
    let quality = match quality {
        Some(q) if !format.is_lossy() => {
            eprintln!(
                "warning: --quality {q} is ignored by {} (only JPEG is lossy)",
                format.label()
            );
            q
        }
        Some(q) => q,
        None => DEFAULT_JPEG_QUALITY,
    };
    // A plain string plus a match rather than a `clap::ValueEnum` derive, so `raw-decode`
    // does not gain a `clap` dependency for a CLI concern. The error lists the valid values,
    // which is what the derive would have bought.
    let demosaic = match demosaic.to_ascii_lowercase().as_str() {
        "fastest" => raw_decode::Demosaic::Fastest,
        "fast" => raw_decode::Demosaic::Fast,
        "better" => raw_decode::Demosaic::Better,
        "best" => raw_decode::Demosaic::Best,
        other => anyhow::bail!("unknown demosaic {other:?} (fastest|fast|better|best)"),
    };
    let decoded = raw_decode::decode_raw(
        input,
        &raw_decode::DecodeOptions {
            demosaic,
            ..raw_decode::DecodeOptions::linear_scene()
        },
    )
    .with_context(|| format!("decoding {}", input.display()))?;
    let m = &decoded.metadata;
    eprintln!("Decoded {}x{}  {} {}", m.width, m.height, m.make, m.model);

    let edits = match edits_path {
        Some(p) => edit_model::Edits::from_json_file(p)
            .with_context(|| format!("loading edits {}", p.display()))?,
        None => edit_model::Edits::default(),
    };
    // Resolve a lens profile, if the file names a lens we know and any correction is wanted.
    // ⚠ `Catalogue::bundled()` decompresses 56 XML files, so it is built once here and never
    // inside a loop. A miss is an ordinary answer, not an error: the stage is simply skipped.
    let lens = if edits.lens_correction.is_active() {
        decoded.metadata.lens.as_ref().and_then(|l| {
            let db = lens_profile::Catalogue::bundled().ok()?;
            let p = db.find(&decoded.metadata.make, &decoded.metadata.model, &l.name)?;
            Some(lens_profile::LensShot {
                profile: p,
                shot: lens_profile::Shot {
                    focal: l.focal,
                    aperture: l.aperture,
                },
            })
        })
    } else {
        None
    };
    if edits.lens_correction.is_active() {
        match (&decoded.metadata.lens, &lens) {
            (Some(l), Some(_)) => {
                eprintln!("Lens profile: {} ({} mm f/{})", l.name, l.focal, l.aperture)
            }
            (Some(l), None) => eprintln!("No lens profile for {:?} — corrections skipped", l.name),
            (None, _) => eprintln!("No lens recorded in the file — corrections skipped"),
        }
    }

    let (prefix, tail) = pipeline::build_pipeline_split_in_region_with_brush(
        &edits,
        pipeline::RenderScale::FULL,
        None,
        None,
        space,
        lens,
    );
    let mut stages = prefix.into_stages();
    stages.extend(tail.into_stages());
    let rendered = pipeline::Pipeline::new(stages).run(decoded.image);

    export::save(&rendered, output, format, quality, space)
        .with_context(|| format!("writing {}", output.display()))?;
    eprintln!("Wrote {}", output.display());
    Ok(())
}
