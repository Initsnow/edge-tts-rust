use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use edge_tts_rust::{
    Boundary, DEFAULT_OUTPUT_FORMAT, EdgeTtsClient, SpeakOptions, default_extension_for_format,
};

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[arg(long)]
    text: Option<String>,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long, default_value = "en-US-EmmaMultilingualNeural")]
    voice: String,
    #[arg(long, alias = "format", default_value = DEFAULT_OUTPUT_FORMAT)]
    output_format: String,
    #[arg(long, default_value = "+0%")]
    rate: String,
    #[arg(long, default_value = "+0%")]
    volume: String,
    #[arg(long, default_value = "+0Hz")]
    pitch: String,
    #[arg(long, value_enum, default_value = "sentence")]
    boundary: BoundaryArg,
    #[arg(long)]
    list_voices: bool,
    #[arg(long)]
    write_media: Option<PathBuf>,
    #[arg(long)]
    write_subtitles: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BoundaryArg {
    Word,
    Sentence,
}

impl From<BoundaryArg> for Boundary {
    fn from(value: BoundaryArg) -> Self {
        match value {
            BoundaryArg::Word => Boundary::Word,
            BoundaryArg::Sentence => Boundary::Sentence,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let client = EdgeTtsClient::new()?;

    if cli.list_voices {
        for voice in client.list_voices().await? {
            println!(
                "{}\t{}\t{}\t{}",
                voice.short_name, voice.locale, voice.gender, voice.name
            );
        }
        return Ok(());
    }

    let text = match (cli.text, cli.file) {
        (Some(text), None) => text,
        (None, Some(path)) => tokio::fs::read_to_string(path).await?,
        _ => return Err("provide exactly one of --text or --file".into()),
    };

    let options = SpeakOptions {
        voice: cli.voice,
        rate: cli.rate,
        volume: cli.volume,
        pitch: cli.pitch,
        boundary: cli.boundary.into(),
        output_format: cli.output_format,
    };

    let default_media_name = format!(
        "output.{}",
        default_extension_for_format(&options.output_format)
    );
    let media = cli
        .write_media
        .unwrap_or_else(|| PathBuf::from(default_media_name));
    client
        .save(text, options, media, cli.write_subtitles)
        .await?;
    Ok(())
}
