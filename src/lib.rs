mod client;
mod constants;
mod error;
mod options;
mod protocol;
mod subtitles;
mod types;

pub use client::{EdgeTtsClient, EdgeTtsClientBuilder, EventStream, subtitles};
pub use error::{Error, Result};
pub use options::{SpeakOptions, normalize_voice};
pub use protocol::{build_ssml, parse_binary_headers, parse_headers, parse_metadata, split_text};
pub use subtitles::to_srt;
pub use types::{Boundary, BoundaryEvent, SynthesisEvent, SynthesisResult, Voice, VoiceTag};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        build_ssml, offset_from_audio_bytes, parse_headers, parse_metadata, split_text,
    };

    #[test]
    fn normalizes_short_voice_name() {
        let normalized = normalize_voice("en-US-EmmaMultilingualNeural").unwrap();
        assert_eq!(
            normalized,
            "Microsoft Server Speech Text to Speech Voice (en-US, EmmaMultilingualNeural)"
        );
    }

    #[test]
    fn preserves_extended_region_voice_name() {
        let normalized = normalize_voice("zh-CN-liaoning-XiaobeiNeural").unwrap();
        assert_eq!(
            normalized,
            "Microsoft Server Speech Text to Speech Voice (zh-CN-liaoning, XiaobeiNeural)"
        );
    }

    #[test]
    fn rejects_invalid_pitch() {
        let options = SpeakOptions {
            pitch: "fast".to_owned(),
            ..SpeakOptions::default()
        };
        assert!(matches!(options.validate(), Err(Error::InvalidPitch(_))));
    }

    #[test]
    fn splits_text_without_breaking_entities() {
        let chunks = split_text("hello & goodbye across the entity boundary", 15).unwrap();
        assert!(chunks.iter().all(|chunk| !chunk.ends_with('&')));
    }

    #[test]
    fn builds_valid_ssml() {
        let ssml = build_ssml(
            "Microsoft Server Speech Text to Speech Voice (en-US, EmmaMultilingualNeural)",
            "+0%",
            "+0%",
            "+0Hz",
            "Fish & Chips",
        );
        assert!(ssml.contains("Fish &amp; Chips"));
    }

    #[test]
    fn parses_text_headers() {
        let frame = b"Path:audio.metadata\r\nX-RequestId:abc\r\n\r\n{\"Metadata\":[]}";
        let header_end = frame
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        let (headers, payload) = parse_headers(frame, header_end).unwrap();
        assert_eq!(headers.get("Path").unwrap(), "audio.metadata");
        assert_eq!(payload, br#"{"Metadata":[]}"#);
    }

    #[test]
    fn parses_metadata_events() {
        let payload = br#"{"Metadata":[{"Type":"WordBoundary","Data":{"Offset":100,"Duration":200,"text":{"Text":"hello"}}}]}"#;
        let events = parse_metadata(payload, 50).unwrap();
        assert_eq!(
            events,
            vec![SynthesisEvent::Boundary(BoundaryEvent {
                kind: Boundary::Word,
                offset_ticks: 150,
                duration_ticks: 200,
                text: "hello".to_owned(),
            })]
        );
    }

    #[test]
    fn compensates_offsets_from_audio_length() {
        assert_eq!(offset_from_audio_bytes(6_000), 10_000_000);
    }

    #[test]
    fn renders_srt() {
        let srt = to_srt(&[BoundaryEvent {
            kind: Boundary::Sentence,
            offset_ticks: 0,
            duration_ticks: 15_000_000,
            text: "hello".to_owned(),
        }]);
        assert!(srt.contains("00:00:00,000 --> 00:00:01,500"));
    }
}
