use edge_tts_rust::{Boundary, EdgeTtsClient, SpeakOptions};

#[tokio::main]
async fn main() -> edge_tts_rust::Result<()> {
    let client = EdgeTtsClient::new()?;
    client
        .save(
            "Hello from Rust",
            SpeakOptions {
                voice: "en-US-EmmaMultilingualNeural".into(),
                boundary: Boundary::Sentence,
                ..SpeakOptions::default()
            },
            "/tmp/example-edge-tts.mp3",
            Some("/tmp/example-edge-tts.srt"),
        )
        .await?;
    Ok(())
}
