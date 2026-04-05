use std::time::{Duration, Instant};

use edge_tts_rust::{Boundary, EdgeTtsClient, SpeakOptions, SynthesisEvent, split_text};
use futures_util::StreamExt;

const DEFAULT_LONG_CONNECTION_ITERATIONS: usize = 40;
const DEFAULT_LONG_TEXT_ITERATIONS: usize = 2;
const DEFAULT_LONG_TEXT_SECTIONS: usize = 24;
const LONG_TEXT_MIN_CHUNKS: usize = 4;

#[derive(Debug)]
struct RepeatedSynthesisStats {
    requests: usize,
    total: Duration,
    average: Duration,
    p50: Duration,
    p95: Duration,
    max: Duration,
    audio_bytes: usize,
    boundaries: usize,
}

#[derive(Debug)]
struct StreamRequestStats {
    first_audio: Duration,
    total: Duration,
    audio_bytes: usize,
    boundaries: usize,
    audio_events: usize,
}

#[derive(Debug)]
struct StreamBenchmarkStats {
    runs: usize,
    text_bytes: usize,
    text_chunks: usize,
    first_audio_avg: Duration,
    first_audio_p50: Duration,
    first_audio_p95: Duration,
    first_audio_max: Duration,
    total_avg: Duration,
    total_p50: Duration,
    total_p95: Duration,
    total_max: Duration,
    audio_bytes: usize,
    boundaries: usize,
    audio_events: usize,
}

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

#[tokio::test]
async fn live_repeated_synthesis_reports_connection_reuse_metrics() -> edge_tts_rust::Result<()> {
    if std::env::var_os("EDGE_TTS_ONLINE_TEST").is_none()
        || std::env::var_os("EDGE_TTS_LONG_CONNECTION_TEST").is_none()
    {
        return Ok(());
    }

    let iterations = long_connection_iterations();
    let texts = benchmark_texts(iterations);
    let options = benchmark_options();

    let pooled_client = EdgeTtsClient::builder()
        .ws_pool_size(1)
        .ws_idle_ttl(Duration::from_secs(60))
        .ws_warmup(false)
        .request_chunk_reuse(true)
        .build()?;
    warmup_client(&pooled_client, &options).await?;
    let pooled = run_repeated_synthesis(&pooled_client, &texts, &options).await?;

    let fresh_client = EdgeTtsClient::builder()
        .ws_pool_size(0)
        .ws_warmup(false)
        .request_chunk_reuse(false)
        .build()?;
    warmup_client(&fresh_client, &options).await?;
    let fresh = run_repeated_synthesis(&fresh_client, &texts, &options).await?;

    print_stats("pooled", &pooled);
    print_stats("fresh", &fresh);
    let speedup = fresh.total.as_secs_f64() / pooled.total.as_secs_f64();
    let saved_ms = (fresh.total.as_secs_f64() - pooled.total.as_secs_f64()) * 1000.0;
    println!("[long-connection] speedup_vs_fresh={speedup:.2}x total_saved_ms={saved_ms:.1}");

    assert_eq!(pooled.requests, iterations);
    assert_eq!(fresh.requests, iterations);
    Ok(())
}

#[tokio::test]
async fn live_long_text_stream_latency_reports_chunk_reuse_metrics() -> edge_tts_rust::Result<()> {
    if std::env::var_os("EDGE_TTS_ONLINE_TEST").is_none()
        || std::env::var_os("EDGE_TTS_LONG_TEXT_TEST").is_none()
    {
        return Ok(());
    }

    let iterations = long_text_iterations();
    let text = long_form_text();
    let text_chunks = split_text(&text, 4096)?.len();
    assert!(
        text_chunks >= LONG_TEXT_MIN_CHUNKS,
        "expected long-form input to span at least {LONG_TEXT_MIN_CHUNKS} chunks, got {text_chunks}"
    );

    let options = benchmark_options();
    let reused_client = EdgeTtsClient::builder()
        .ws_pool_size(0)
        .ws_warmup(false)
        .request_chunk_reuse(true)
        .build()?;
    let fresh_client = EdgeTtsClient::builder()
        .ws_pool_size(0)
        .ws_warmup(false)
        .request_chunk_reuse(false)
        .build()?;

    warmup_client(&reused_client, &options).await?;
    warmup_client(&fresh_client, &options).await?;

    let mut reused_runs = Vec::with_capacity(iterations);
    let mut fresh_runs = Vec::with_capacity(iterations);

    for round in 0..iterations {
        if round % 2 == 0 {
            reused_runs.push(run_stream_request(&reused_client, &text, &options).await?);
            fresh_runs.push(run_stream_request(&fresh_client, &text, &options).await?);
        } else {
            fresh_runs.push(run_stream_request(&fresh_client, &text, &options).await?);
            reused_runs.push(run_stream_request(&reused_client, &text, &options).await?);
        }
    }

    let reused = summarize_stream_runs(text.len(), text_chunks, &reused_runs);
    let fresh = summarize_stream_runs(text.len(), text_chunks, &fresh_runs);

    print_stream_stats("reused_chunks", &reused);
    print_stream_stats("fresh_per_chunk", &fresh);
    println!(
        "[long-text] first_audio_speedup_vs_fresh={:.2}x total_speedup_vs_fresh={:.2}x",
        fresh.first_audio_avg.as_secs_f64() / reused.first_audio_avg.as_secs_f64(),
        fresh.total_avg.as_secs_f64() / reused.total_avg.as_secs_f64(),
    );

    assert_eq!(reused.runs, iterations);
    assert_eq!(fresh.runs, iterations);
    Ok(())
}

fn benchmark_options() -> SpeakOptions {
    SpeakOptions {
        voice: "en-US-EmmaMultilingualNeural".into(),
        boundary: Boundary::Sentence,
        ..SpeakOptions::default()
    }
}

fn long_connection_iterations() -> usize {
    std::env::var("EDGE_TTS_LONG_CONNECTION_ITERATIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_LONG_CONNECTION_ITERATIONS)
}

fn long_text_iterations() -> usize {
    std::env::var("EDGE_TTS_LONG_TEXT_ITERATIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_LONG_TEXT_ITERATIONS)
}

fn long_text_sections() -> usize {
    std::env::var("EDGE_TTS_LONG_TEXT_SECTIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_LONG_TEXT_SECTIONS)
}

fn benchmark_texts(iterations: usize) -> Vec<String> {
    let corpus = [
        "Rust keeps the transport stable while the service keeps speaking.",
        "This request is part of a repeated synthesis run over a reused websocket.",
        "Short inputs are useful for isolating connection setup overhead from synthesis work.",
        "Sentence boundaries should still be present even when the same client is used repeatedly.",
        "The benchmark mixes content length slightly so the run is not a single repeated payload.",
    ];

    (0..iterations)
        .map(|index| {
            format!(
                "request {} of {}. {}",
                index + 1,
                iterations,
                corpus[index % corpus.len()]
            )
        })
        .collect()
}

fn long_form_text() -> String {
    let paragraph = concat!(
        "The rain had stopped before dawn, but the town still carried the smell of wet stone, ",
        "cold wood, and the iron rails by the river. ",
        "When the first carts reached the market square, every wheel struck the same shallow puddles, ",
        "and each splash seemed to wake another window. ",
        "No one said the morning was unusual, yet everyone walked as though they had been given the same quiet instruction. ",
        "Move carefully. Listen closely. Wait for something to announce itself. "
    );

    (0..long_text_sections())
        .map(|chapter| {
            format!(
                "Chapter {}. {} The clerk unfolded a letter, read it once, and then read it again as if the second reading could change the facts. ",
                chapter + 1,
                paragraph
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

async fn warmup_client(
    client: &EdgeTtsClient,
    options: &SpeakOptions,
) -> edge_tts_rust::Result<()> {
    let result = client
        .synthesize(
            "warmup request for repeated synthesis benchmarking",
            options.clone(),
        )
        .await?;
    assert!(
        !result.audio.is_empty(),
        "warmup synthesis produced no audio"
    );
    Ok(())
}

async fn run_stream_request(
    client: &EdgeTtsClient,
    text: &str,
    options: &SpeakOptions,
) -> edge_tts_rust::Result<StreamRequestStats> {
    let started = Instant::now();
    let mut stream = client.stream(text.to_owned(), options.clone()).await?;
    let mut first_audio = None;
    let mut audio_bytes = 0usize;
    let mut boundaries = 0usize;
    let mut audio_events = 0usize;

    while let Some(event) = stream.next().await {
        match event? {
            SynthesisEvent::Audio(chunk) => {
                if first_audio.is_none() {
                    first_audio = Some(started.elapsed());
                }
                audio_events += 1;
                audio_bytes += chunk.len();
            }
            SynthesisEvent::Boundary(_) => boundaries += 1,
        }
    }

    Ok(StreamRequestStats {
        first_audio: first_audio.expect("stream produced audio"),
        total: started.elapsed(),
        audio_bytes,
        boundaries,
        audio_events,
    })
}

async fn run_repeated_synthesis(
    client: &EdgeTtsClient,
    texts: &[String],
    options: &SpeakOptions,
) -> edge_tts_rust::Result<RepeatedSynthesisStats> {
    let started = Instant::now();
    let mut latencies = Vec::with_capacity(texts.len());
    let mut audio_bytes = 0usize;
    let mut boundaries = 0usize;

    for text in texts {
        let request_started = Instant::now();
        let result = client.synthesize(text.clone(), options.clone()).await?;
        let elapsed = request_started.elapsed();

        assert!(!result.audio.is_empty(), "synthesis produced no audio");
        audio_bytes += result.audio.len();
        boundaries += result.boundaries.len();
        latencies.push(elapsed);
    }

    latencies.sort_unstable();

    let total = started.elapsed();
    let requests = latencies.len();
    let average = Duration::from_secs_f64(total.as_secs_f64() / requests as f64);
    let p50 = percentile(&latencies, 0.50);
    let p95 = percentile(&latencies, 0.95);
    let max = *latencies.last().expect("latencies populated");

    Ok(RepeatedSynthesisStats {
        requests,
        total,
        average,
        p50,
        p95,
        max,
        audio_bytes,
        boundaries,
    })
}

fn percentile(sorted: &[Duration], ratio: f64) -> Duration {
    let last_index = sorted.len().saturating_sub(1);
    let index = ((last_index as f64) * ratio).round() as usize;
    sorted[index.min(last_index)]
}

fn average_duration(values: &[Duration]) -> Duration {
    Duration::from_secs_f64(
        values.iter().map(Duration::as_secs_f64).sum::<f64>() / values.len() as f64,
    )
}

fn summarize_stream_runs(
    text_bytes: usize,
    text_chunks: usize,
    runs: &[StreamRequestStats],
) -> StreamBenchmarkStats {
    let mut first_audio = runs.iter().map(|run| run.first_audio).collect::<Vec<_>>();
    let mut total = runs.iter().map(|run| run.total).collect::<Vec<_>>();
    first_audio.sort_unstable();
    total.sort_unstable();

    StreamBenchmarkStats {
        runs: runs.len(),
        text_bytes,
        text_chunks,
        first_audio_avg: average_duration(&first_audio),
        first_audio_p50: percentile(&first_audio, 0.50),
        first_audio_p95: percentile(&first_audio, 0.95),
        first_audio_max: *first_audio.last().expect("first-audio samples populated"),
        total_avg: average_duration(&total),
        total_p50: percentile(&total, 0.50),
        total_p95: percentile(&total, 0.95),
        total_max: *total.last().expect("total samples populated"),
        audio_bytes: runs.iter().map(|run| run.audio_bytes).sum(),
        boundaries: runs.iter().map(|run| run.boundaries).sum(),
        audio_events: runs.iter().map(|run| run.audio_events).sum(),
    }
}

fn print_stats(label: &str, stats: &RepeatedSynthesisStats) {
    println!(
        "[long-connection] mode={label} requests={} total_ms={:.1} avg_ms={:.1} p50_ms={:.1} p95_ms={:.1} max_ms={:.1} audio_bytes={} boundaries={}",
        stats.requests,
        stats.total.as_secs_f64() * 1000.0,
        stats.average.as_secs_f64() * 1000.0,
        stats.p50.as_secs_f64() * 1000.0,
        stats.p95.as_secs_f64() * 1000.0,
        stats.max.as_secs_f64() * 1000.0,
        stats.audio_bytes,
        stats.boundaries,
    );
}

fn print_stream_stats(label: &str, stats: &StreamBenchmarkStats) {
    println!(
        "[long-text] mode={label} runs={} text_bytes={} text_chunks={} first_audio_avg_ms={:.1} first_audio_p50_ms={:.1} first_audio_p95_ms={:.1} first_audio_max_ms={:.1} total_avg_ms={:.1} total_p50_ms={:.1} total_p95_ms={:.1} total_max_ms={:.1} audio_bytes={} boundaries={} audio_events={}",
        stats.runs,
        stats.text_bytes,
        stats.text_chunks,
        stats.first_audio_avg.as_secs_f64() * 1000.0,
        stats.first_audio_p50.as_secs_f64() * 1000.0,
        stats.first_audio_p95.as_secs_f64() * 1000.0,
        stats.first_audio_max.as_secs_f64() * 1000.0,
        stats.total_avg.as_secs_f64() * 1000.0,
        stats.total_p50.as_secs_f64() * 1000.0,
        stats.total_p95.as_secs_f64() * 1000.0,
        stats.total_max.as_secs_f64() * 1000.0,
        stats.audio_bytes,
        stats.boundaries,
        stats.audio_events,
    );
}
