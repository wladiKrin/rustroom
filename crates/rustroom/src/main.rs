use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

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
    /// Decode a RAW, run the develop pipeline (no edits yet), and export a JPEG.
    Render {
        /// Input RAW file.
        input: PathBuf,
        /// Output JPEG.
        output: PathBuf,
        /// Optional edits sidecar (JSON). Omitted = no edits.
        #[arg(long)]
        edits: Option<PathBuf>,
        /// JPEG quality 1..=100.
        #[arg(long, default_value_t = 92)]
        quality: u8,
        /// Demosaic quality: fastest, fast, better, best. Higher is slower — roughly 1.2 s
        /// to 11 s per photo on a 26 Mpx RAW.
        #[arg(long, default_value = "fast")]
        demosaic: String,
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
            quality,
            demosaic,
        } => render(&input, &output, edits.as_deref(), quality, &demosaic),
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
    quality: u8,
    demosaic: &str,
) -> Result<()> {
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
    let rendered = pipeline::build_pipeline(&edits, pipeline::RenderScale::FULL).run(decoded.image);

    export::save(&rendered, output, export::ExportFormat::Jpeg, quality)
        .with_context(|| format!("writing {}", output.display()))?;
    eprintln!("Wrote {}", output.display());
    Ok(())
}
