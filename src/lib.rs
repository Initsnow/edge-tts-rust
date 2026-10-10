mod client;
mod constants;
mod error;
mod options;
mod protocol;
mod subtitles;
mod types;

pub use client::{EdgeTtsClient, EdgeTtsClientBuilder, EventStream, subtitles};
pub use constants::{
    DEFAULT_OUTPUT_FORMAT, DEFAULT_OUTPUT_FORMAT_MP3, DEFAULT_OUTPUT_FORMAT_OPUS, DEFAULT_VOICE,
    OUTPUT_FORMAT,
};
pub use error::{Error, Result};
pub use options::{default_extension_for_format, normalize_voice, SpeakOptions};
pub use protocol::{
    build_ssml, cbr_bitrate_bps, offset_from_audio_bytes, offset_from_audio_bytes_with_bitrate,
    parse_binary_headers, parse_headers, parse_metadata, speech_config_message, split_text,
};
pub use subtitles::to_srt;
pub use types::{Boundary, BoundaryEvent, SynthesisEvent, SynthesisResult, Voice, VoiceTag};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        build_ssml, cbr_bitrate_bps, offset_from_audio_bytes, parse_headers, parse_metadata,
        speech_config_message, split_text,
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

    #[test]
    fn default_format_is_opus() {
        assert_eq!(DEFAULT_OUTPUT_FORMAT, "webm-24khz-16bit-mono-opus");
        let options = SpeakOptions::default();
        assert_eq!(options.output_format, "webm-24khz-16bit-mono-opus");
        assert_eq!(default_extension_for_format(&options.output_format), "webm");
    }

    #[test]
    fn custom_output_format_in_speech_config() {
        let msg = speech_config_message(Boundary::Sentence, "audio-24khz-96kbitrate-mono-mp3");
        assert!(msg.contains("\"outputFormat\":\"audio-24khz-96kbitrate-mono-mp3\""));
    }

    #[test]
    fn parses_cbr_bitrates_correctly() {
        assert_eq!(cbr_bitrate_bps("audio-24khz-48kbitrate-mono-mp3"), Some(48_000));
        assert_eq!(cbr_bitrate_bps("audio-24khz-96kbitrate-mono-mp3"), Some(96_000));
        assert_eq!(cbr_bitrate_bps("webm-24khz-16bit-mono-opus"), None);
    }

    #[test]
    fn resolves_default_extensions() {
        assert_eq!(default_extension_for_format("webm-24khz-16bit-mono-opus"), "webm");
        assert_eq!(default_extension_for_format("audio-24khz-48kbitrate-mono-mp3"), "mp3");
        assert_eq!(default_extension_for_format("ogg-24khz-16bit-mono-opus"), "opus");
        assert_eq!(default_extension_for_format("riff-24khz-16bit-mono-pcm"), "wav");
    }
}
