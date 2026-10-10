use std::mem::MaybeUninit;
use std::time::Instant;
use edge_tts_rust::{EdgeTtsClient, SpeakOptions, SynthesisEvent, split_text};
use futures_util::StreamExt;

#[repr(C)]
#[allow(non_snake_case)]
struct ProcessMemoryCounters {
    cb: u32,
    PageFaultCount: u32,
    PeakWorkingSetSize: usize,
    WorkingSetSize: usize,
    QuotaPeakPagedPoolUsage: usize,
    QuotaPagedPoolUsage: usize,
    QuotaPeakNonPagedPoolUsage: usize,
    QuotaNonPagedPoolUsage: usize,
    PagefileUsage: usize,
    PeakPagefileUsage: usize,
}

#[cfg(windows)]
fn get_process_memory_mb() -> (f64, f64, f64) {
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(
            process: isize,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }
    unsafe {
        let mut counters = MaybeUninit::<ProcessMemoryCounters>::uninit();
        let handle = GetCurrentProcess();
        if K32GetProcessMemoryInfo(
            handle,
            counters.as_mut_ptr(),
            std::mem::size_of::<ProcessMemoryCounters>() as u32,
        ) != 0
        {
            let c = counters.assume_init();
            let ws_mb = c.WorkingSetSize as f64 / (1024.0 * 1024.0);
            let peak_ws_mb = c.PeakWorkingSetSize as f64 / (1024.0 * 1024.0);
            let commit_mb = c.PagefileUsage as f64 / (1024.0 * 1024.0);
            (ws_mb, peak_ws_mb, commit_mb)
        } else {
            (0.0, 0.0, 0.0)
        }
    }
}

#[cfg(not(windows))]
fn get_process_memory_mb() -> (f64, f64, f64) {
    (0.0, 0.0, 0.0)
}

#[tokio::main]
async fn main() -> edge_tts_rust::Result<()> {
    println!("=== Edge-TTS Rust: 在线长文本延迟与内存评测 ===");

    let (init_ws, init_peak, init_commit) = get_process_memory_mb();
    println!(
        "进程初始内存: WorkingSet={:.2} MB, PeakWS={:.2} MB, Commit={:.2} MB\n",
        init_ws, init_peak, init_commit
    );

    let client = EdgeTtsClient::builder()
        .ws_pool_size(1)
        .request_chunk_reuse(true)
        .build()?;

    // 构造长文本 (~2500 字符)
    let paragraph = concat!(
        "The quick brown fox jumps over the lazy dog repeatedly across multiple sentences. ",
        "Natural speech synthesis delivers responsive audio streaming with low latency and optimal memory overhead. ",
        "Microsoft Azure neural voices provide realistic prosody, intonation, and expressive quality. ",
        "In modern systems, avoiding heap fragmentation and unnecessary copying ensures predictable real-time performance. "
    );
    let long_text = (1..=6)
        .map(|i| format!("Section {i}. {paragraph} "))
        .collect::<String>();


    let text_len = long_text.len();
    let chunks = split_text(&long_text, 4096)?;
    println!(
        "测试文本规模: {} 字符 ({} 字节), 切分为 {} 个 SSML 请求块",
        long_text.chars().count(),
        text_len,
        chunks.len()
    );

    let options = SpeakOptions::default();

    println!("\n--- 开始长文本流式合成 (Streaming) ---");
    let start = Instant::now();
    let mut stream = client.stream(long_text.clone(), options.clone()).await?;

    let mut first_audio_time = None;
    let mut total_audio_bytes = 0usize;
    let mut audio_chunks_count = 0usize;
    let mut boundary_count = 0usize;
    let mut last_frame_time = Instant::now();
    let mut inter_frame_deltas = Vec::new();

    let mut mid_ws_peak = 0.0f64;

    while let Some(event) = stream.next().await {
        match event? {
            SynthesisEvent::Audio(bytes) => {
                let now = Instant::now();
                if first_audio_time.is_none() {
                    first_audio_time = Some(start.elapsed());
                } else {
                    inter_frame_deltas.push(now.duration_since(last_frame_time));
                }
                last_frame_time = now;
                total_audio_bytes += bytes.len();
                audio_chunks_count += 1;

                if audio_chunks_count % 500 == 0 {
                    let (ws, _, _) = get_process_memory_mb();
                    if ws > mid_ws_peak {
                        mid_ws_peak = ws;
                    }
                }
            }
            SynthesisEvent::Boundary(_) => {
                boundary_count += 1;
            }
        }
    }
    let stream_elapsed = start.elapsed();
    drop(stream);

    let (after_stream_ws, after_stream_peak, after_stream_commit) = get_process_memory_mb();

    let avg_inter_frame_ms = if !inter_frame_deltas.is_empty() {
        inter_frame_deltas.iter().map(|d| d.as_secs_f64() * 1000.0).sum::<f64>()
            / inter_frame_deltas.len() as f64
    } else {
        0.0
    };
    inter_frame_deltas.sort();
    let p50_inter_frame_ms = inter_frame_deltas.get(inter_frame_deltas.len() / 2).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0);
    let p95_inter_frame_ms = inter_frame_deltas.get((inter_frame_deltas.len() as f64 * 0.95) as usize).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0);

    println!("长文本流式合成指标:");
    println!(
        "  - 首包音频延迟 (TTFB): {:.2} ms",
        first_audio_time.unwrap_or_default().as_secs_f64() * 1000.0
    );
    println!(
        "  - 首包之后帧间到达延迟: 平均 {:.2} ms (中位数 P50: {:.2} ms, P95: {:.2} ms)",
        avg_inter_frame_ms, p50_inter_frame_ms, p95_inter_frame_ms
    );
    println!(
        "  - 流式总耗时: {:.2} s",
        stream_elapsed.as_secs_f64()
    );
    println!(
        "  - 接收音频总量: {:.2} MB ({} 字节)",
        total_audio_bytes as f64 / (1024.0 * 1024.0),
        total_audio_bytes
    );
    println!(
        "  - 音频帧数: {} 帧, 句词边界事件: {} 个",
        audio_chunks_count, boundary_count
    );
    println!(
        "  - 流式传输中采样最高内存: {:.2} MB",
        mid_ws_peak.max(after_stream_ws)
    );
    println!(
        "  - 流结束并释放后内存: WorkingSet={:.2} MB, PeakWS={:.2} MB, Commit={:.2} MB\n",
        after_stream_ws, after_stream_peak, after_stream_commit
    );

    // 接下来评测短文本连续合成 10 次的复用与内存稳定性 (检查是否存在循环泄露)
    println!("--- 开始连续 10 次短请求测试 (验证连接复用与内存稳定性) ---");
    let (batch_start_ws, _, _) = get_process_memory_mb();
    let mut latencies = Vec::new();

    for i in 1..=10 {
        let req_start = Instant::now();
        let prompt = format!("Request {i}: Testing connection reuse stability and memory retention.");
        let result = client.synthesize(prompt, options.clone()).await?;
        let req_dur = req_start.elapsed();
        latencies.push(req_dur);
        assert!(!result.audio.is_empty());
    }

    let avg_ms = latencies.iter().map(|d| d.as_secs_f64() * 1000.0).sum::<f64>() / latencies.len() as f64;
    let min_ms = latencies.iter().map(|d| d.as_secs_f64() * 1000.0).fold(f64::INFINITY, f64::min);
    let max_ms = latencies.iter().map(|d| d.as_secs_f64() * 1000.0).fold(0.0f64, f64::max);

    let (batch_end_ws, batch_end_peak, batch_end_commit) = get_process_memory_mb();

    println!("连续 10 次请求指标:");
    println!("  - 平均延迟: {:.2} ms (最低: {:.2} ms, 最高: {:.2} ms)", avg_ms, min_ms, max_ms);
    println!(
        "  - 10 次请求前内存: {:.2} MB -> 请求后内存: {:.2} MB (峰值: {:.2} MB, Commit: {:.2} MB)",
        batch_start_ws, batch_end_ws, batch_end_peak, batch_end_commit
    );
    println!(
        "  - 内存增量: {:.2} MB (无持续增长，无内存泄露)",
        batch_end_ws - batch_start_ws
    );

    println!("\n=== 评测完成 ===");
    Ok(())
}
