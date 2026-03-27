use criterion::{Criterion, criterion_group, criterion_main};
use edge_tts_rust::{build_ssml, split_text};

fn bench_split_text(c: &mut Criterion) {
    let text = "hello from rust ".repeat(1000);
    c.bench_function("split_text_16k", |b| {
        b.iter(|| split_text(&text, 4096).unwrap());
    });
}

fn bench_build_ssml(c: &mut Criterion) {
    c.bench_function("build_ssml", |b| {
        b.iter(|| {
            build_ssml(
                "Microsoft Server Speech Text to Speech Voice (en-US, EmmaMultilingualNeural)",
                "+0%",
                "+0%",
                "+0Hz",
                "hello from rust",
            )
        });
    });
}

criterion_group!(benches, bench_split_text, bench_build_ssml);
criterion_main!(benches);
