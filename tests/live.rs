use edge_tts_rust::{Boundary, EdgeTtsClient, SpeakOptions};

#[tokio::test]
async fn live_synthesis_smoke_test() -> edge_tts_rust::Result<()> {
    if std::env::var_os("EDGE_TTS_ONLINE_TEST").is_none() {
        return Ok(());
    }

    let client = EdgeTtsClient::new()?;
    let result = client
        .synthesize(
            "hello from rust",
            SpeakOptions {
                voice: "en-US-EmmaMultilingualNeural".into(),
                boundary: Boundary::Sentence,
                ..SpeakOptions::default()
            },
        )
        .await?;

    assert!(!result.audio.is_empty());
    assert!(!result.boundaries.is_empty());
    Ok(())
}
