# edge-tts-rust

高性能、异步优先的 Microsoft Edge Read Aloud TTS Rust 客户端。

## 特性

- 异步流式合成，适合长文本和服务端场景
- 支持语音列表、语速、音量、音高、词/句边界时间戳
- 提供 `lib` API 和简单 CLI
- 单元测试、集成测试和可选联机测试覆盖协议编码、文本切分、时间戳解析和字幕生成

## 安装

```bash
cargo build --release
```

## CLI

```bash
cargo run -- --text "你好，世界" --voice zh-CN-XiaoxiaoNeural --write-media out.mp3 --write-subtitles out.srt
```

列出语音：

```bash
cargo run -- --list-voices
```

## 库用法

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

流式使用：

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

## 测试

```bash
cargo test
```

运行可选联机测试：

```bash
EDGE_TTS_ONLINE_TEST=1 cargo test --test live
```

运行基准：

```bash
cargo bench
```

## 设计说明

- `EdgeTtsClient::stream` 返回流式事件，避免大文本一次性堆内存
- `EdgeTtsClient::synthesize` 在流式接口之上做聚合，便于脚本和批处理
- 文本切分会避免截断 UTF-8 和 XML 实体
- 字幕时间补偿基于实际音频字节数，降低长文本偏移漂移
