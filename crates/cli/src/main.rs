use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "oslr", about = "Open-source Lightroom — CLI harness")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decode a RAW file and write it as an 8-bit TIFF.
    Decode {
        /// Input RAW file.
        input: PathBuf,
        /// Output image (.tiff).
        output: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Decode { input, output } => decode(&input, &output),
    }
}

fn decode(input: &Path, output: &Path) -> Result<()> {
    let decoded =
        raw_decode::decode_raw(input).with_context(|| format!("decoding {}", input.display()))?;
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
