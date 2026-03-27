use edge_tts_rust::EdgeTtsClient;

#[tokio::main]
async fn main() -> edge_tts_rust::Result<()> {
    let client = EdgeTtsClient::new()?;
    let voices = client.list_voices().await?;

    for voice in voices.iter().take(10) {
        println!(
            "{}\t{}\t{}\t{}",
            voice.short_name, voice.locale, voice.gender, voice.name
        );
    }

    Ok(())
}
