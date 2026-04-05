# edge-tts-rust

High-performance, async-first Rust client for Microsoft Edge Read Aloud TTS.

## Features

- Async streaming synthesis for long text and backend workloads
- Warm WebSocket pooling with idle TTL and request-level chunk reuse
- Voice listing, rate, volume, pitch, and word/sentence boundary timestamps
- Library API and simple CLI
- Unit tests, integration tests, and optional live tests

## API

- `EdgeTtsClient::new()` enables the default pooled transport
- `EdgeTtsClient::builder()` lets you tune timeouts, warmup, pool size, idle TTL, and chunk reuse
- `list_voices()`, `stream()`, `synthesize()`, and `save()` cover the main client workflows
- `normalize_voice()` converts short voice names into the full Edge voice identifier

## Build

```bash
cargo build --release
```

## CLI

```bash
cargo run -- --text "Hello, world" --voice en-US-EmmaMultilingualNeural --write-media out.mp3 --write-subtitles out.srt
```

List voices:

```bash
cargo run -- --list-voices
```

## Library Usage

```rust
use edge_tts_rust::{Boundary, EdgeTtsClient, SpeakOptions};

#[tokio::main]
async fn main() -> edge_tts_rust::Result<()> {
    let client = EdgeTtsClient::new()?;
    let result = client
        .synthesize(
            "Hello from Rust",
            SpeakOptions {
                voice: "en-US-EmmaMultilingualNeural".into(),
                boundary: Boundary::Sentence,
                ..SpeakOptions::default()
            },
        )
        .await?;

    println!("audio bytes: {}", result.audio.len());
    println!("boundaries: {}", result.boundaries.len());
    Ok(())
}
```

Streaming:

```rust
use edge_tts_rust::{EdgeTtsClient, SpeakOptions, SynthesisEvent};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> edge_tts_rust::Result<()> {
    let client = EdgeTtsClient::new()?;
    let mut stream = client
        .stream("hello from rust", SpeakOptions::default())
        .await?;

    while let Some(event) = stream.next().await {
        match event? {
            SynthesisEvent::Audio(chunk) => println!("audio chunk: {}", chunk.len()),
            SynthesisEvent::Boundary(boundary) => println!("boundary: {}", boundary.text),
        }
    }
    Ok(())
}
```

Transport tuning:

```rust
use std::time::Duration;

use edge_tts_rust::EdgeTtsClient;

let client = EdgeTtsClient::builder()
    .ws_pool_size(2)
    .ws_idle_ttl(Duration::from_secs(15))
    .ws_warmup(true)
    .request_chunk_reuse(true)
    .build()?;
```

The default client already enables pooled WebSocket warmup and chunk reuse.

## Testing

```bash
cargo test
```

Run the optional live test:

```bash
EDGE_TTS_ONLINE_TEST=1 cargo test --test live
```

Run the repeated-request long-connection test and print pooled-vs-fresh metrics:

```bash
EDGE_TTS_ONLINE_TEST=1 EDGE_TTS_LONG_CONNECTION_TEST=1 EDGE_TTS_LONG_CONNECTION_ITERATIONS=50 cargo test --test live -- --nocapture
```

This reports total time plus per-request average, p50, p95, and max latency for:

- `ws_pool_size=1` with connection reuse enabled
- `ws_pool_size=0` with a fresh websocket for each request

Run the long-text streaming test and compare first-audio latency for chunk reuse:

```bash
EDGE_TTS_ONLINE_TEST=1 EDGE_TTS_LONG_TEXT_TEST=1 EDGE_TTS_LONG_TEXT_ITERATIONS=2 EDGE_TTS_LONG_TEXT_SECTIONS=24 cargo test --test live -- --nocapture
```

This uses one long-form text that spans multiple protocol chunks and reports:

- `first_audio_*` latency, which is closer to interactive playback experience
- `total_*` latency for the full synthesis
- `request_chunk_reuse=true` versus `request_chunk_reuse=false`
- `EDGE_TTS_LONG_TEXT_SECTIONS` can scale the test input up or down

Run benchmarks:

```bash
cargo bench
```
