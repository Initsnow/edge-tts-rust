use edge_tts_rust::{
    Boundary, BoundaryEvent, Error, SynthesisEvent, build_ssml, parse_binary_headers,
    parse_headers, parse_metadata, split_text, to_srt,
};

#[test]
fn text_headers_strip_separator() {
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
fn binary_headers_strip_prefix_and_separator() {
    let header = b"Path:audio\r\nContent-Type:audio/mpeg";
    let mut frame = Vec::new();
    frame.extend_from_slice(&(header.len() as u16).to_be_bytes());
    frame.extend_from_slice(header);
    frame.extend_from_slice(b"\r\n");
    frame.extend_from_slice(b"\xff\xfb");
    let (headers, payload) = parse_binary_headers(&frame, header.len()).unwrap();
    assert_eq!(headers.get("Path").unwrap(), "audio");
    assert_eq!(headers.get("Content-Type").unwrap(), "audio/mpeg");
    assert_eq!(payload, b"\xff\xfb");
}

#[test]
fn metadata_unescapes_xml_text() {
    let payload = br#"{"Metadata":[{"Type":"SentenceBoundary","Data":{"Offset":10,"Duration":20,"text":{"Text":"Tom &amp; Jerry"}}}]}"#;
    let events = parse_metadata(payload, 5).unwrap();
    assert_eq!(
        events,
        vec![SynthesisEvent::Boundary(BoundaryEvent {
            kind: Boundary::Sentence,
            offset_ticks: 15,
            duration_ticks: 20,
            text: "Tom & Jerry".into(),
        })]
    );
}

#[test]
fn split_text_rejects_zero_limit() {
    assert!(matches!(
        split_text("hello", 0),
        Err(Error::InvalidChunkSize)
    ));
}

#[test]
fn ssml_escapes_special_characters() {
    let ssml = build_ssml(
        "Microsoft Server Speech Text to Speech Voice (en-US, EmmaMultilingualNeural)",
        "+0%",
        "+0%",
        "+0Hz",
        "a < b & c",
    );
    assert!(ssml.contains("a &lt; b &amp; c"));
}

#[test]
fn srt_output_is_stable() {
    let srt = to_srt(&[BoundaryEvent {
        kind: Boundary::Sentence,
        offset_ticks: 0,
        duration_ticks: 20_000_000,
        text: "hello".into(),
    }]);
    assert_eq!(srt, "1\n00:00:00,000 --> 00:00:02,000\nhello\n\n");
}
