use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::constants::{
    MP3_BITRATE_BPS, OUTPUT_FORMAT, SEC_MS_GEC_VERSION, TICKS_PER_SECOND, TRUSTED_CLIENT_TOKEN,
};
use crate::error::{Error, Result};
use crate::options::{SpeakOptions, normalize_voice};
use crate::types::{Boundary, BoundaryEvent, SynthesisEvent};

const WINDOWS_EPOCH_OFFSET_SECONDS: u64 = 11_644_473_600;

pub fn generate_connection_id() -> String {
    Uuid::new_v4().simple().to_string()
}

pub fn generate_muid() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

pub fn generate_sec_ms_gec(now: SystemTime) -> String {
    let unix_seconds = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let rounded = (unix_seconds + WINDOWS_EPOCH_OFFSET_SECONDS) / 300 * 300;
    let windows_ticks = rounded * 10_000_000;
    let mut hasher = Sha256::new();
    hasher.update(format!("{windows_ticks}{TRUSTED_CLIENT_TOKEN}").as_bytes());
    format!("{:X}", hasher.finalize())
}

pub fn javascript_timestamp() -> String {
    Utc::now()
        .format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)")
        .to_string()
}

pub fn speech_config_message(boundary: Boundary) -> String {
    let (word, sentence) = boundary.metadata_flags();
    let payload = json!({
        "context": {
            "synthesis": {
                "audio": {
                    "metadataoptions": {
                        "sentenceBoundaryEnabled": sentence,
                        "wordBoundaryEnabled": word
                    },
                    "outputFormat": OUTPUT_FORMAT
                }
            }
        }
    });
    format!(
        "X-Timestamp:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{}\r\n",
        javascript_timestamp(),
        payload
    )
}

pub fn ssml_message(options: &SpeakOptions, chunk: &str) -> Result<String> {
    let voice = normalize_voice(&options.voice)?;
    Ok(format!(
        "X-RequestId:{}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{}Z\r\nPath:ssml\r\n\r\n{}",
        generate_connection_id(),
        javascript_timestamp(),
        build_ssml(
            &voice,
            &options.rate,
            &options.volume,
            &options.pitch,
            chunk
        )
    ))
}

pub fn build_ssml(voice: &str, rate: &str, volume: &str, pitch: &str, text: &str) -> String {
    format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'><voice name='{voice}'><prosody pitch='{pitch}' rate='{rate}' volume='{volume}'>{}</prosody></voice></speak>",
        escape_ssml_text(text)
    )
}

pub fn escape_ssml_text(text: &str) -> String {
    let sanitized = text
        .chars()
        .map(|ch| match ch as u32 {
            0..=8 | 11..=12 | 14..=31 => ' ',
            _ => ch,
        })
        .collect::<String>();

    sanitized
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn split_text(text: &str, max_bytes: usize) -> Result<Vec<String>> {
    if max_bytes == 0 {
        return Err(Error::InvalidChunkSize);
    }

    let escaped = escape_ssml_text(text);
    let mut bytes = escaped.as_bytes();
    let mut chunks = Vec::new();

    while bytes.len() > max_bytes {
        let mut split_at = bytes[..max_bytes]
            .iter()
            .rposition(|byte| *byte == b'\n' || *byte == b' ')
            .unwrap_or(max_bytes);

        while std::str::from_utf8(&bytes[..split_at]).is_err() && split_at > 0 {
            split_at -= 1;
        }

        split_at = adjust_entity_boundary(bytes, split_at);
        if split_at == 0 {
            return Err(Error::InvalidSplitPoint);
        }

        let chunk = std::str::from_utf8(&bytes[..split_at])
            .map_err(|_| Error::InvalidSplitPoint)?
            .trim();
        if !chunk.is_empty() {
            chunks.push(chunk.to_owned());
        }
        bytes = &bytes[split_at..];
    }

    let tail = std::str::from_utf8(bytes)
        .map_err(|_| Error::InvalidSplitPoint)?
        .trim();
    if !tail.is_empty() {
        chunks.push(tail.to_owned());
    }

    Ok(chunks)
}

fn adjust_entity_boundary(bytes: &[u8], mut split_at: usize) -> usize {
    while split_at > 0 {
        if let Some(amp_index) = bytes[..split_at].iter().rposition(|byte| *byte == b'&') {
            if bytes[amp_index..split_at].contains(&b';') {
                break;
            }
            split_at = amp_index;
            continue;
        }
        break;
    }
    split_at
}

pub fn parse_headers(
    data: &[u8],
    header_length: usize,
) -> Result<(BTreeMap<String, String>, &[u8])> {
    if header_length > data.len() {
        return Err(Error::UnexpectedResponse(
            "header length exceeds frame length",
        ));
    }

    let header_bytes = &data[..header_length];
    let payload = data
        .get(header_length..)
        .ok_or(Error::UnexpectedResponse("frame missing payload"))?;
    let payload = payload
        .strip_prefix(b"\r\n\r\n")
        .or_else(|| payload.strip_prefix(b"\r\n"))
        .unwrap_or(payload);
    let header_str = std::str::from_utf8(header_bytes)
        .map_err(|_| Error::UnexpectedResponse("headers are not valid utf-8"))?;

    let mut headers = BTreeMap::new();
    for line in header_str.split("\r\n").filter(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(key.to_owned(), value.to_owned());
    }
    Ok((headers, payload))
}

pub fn parse_binary_headers(
    data: &[u8],
    header_length: usize,
) -> Result<(BTreeMap<String, String>, &[u8])> {
    let header_start = 2usize;
    let header_end = header_start
        .checked_add(header_length)
        .ok_or(Error::UnexpectedResponse("binary header length overflow"))?;
    if header_end > data.len() {
        return Err(Error::UnexpectedResponse(
            "binary header length exceeds frame length",
        ));
    }

    let header_bytes = &data[header_start..header_end];
    let payload = data
        .get(header_end..)
        .ok_or(Error::UnexpectedResponse("binary frame missing payload"))?;
    let payload = payload.strip_prefix(b"\r\n").unwrap_or(payload);
    let header_str = std::str::from_utf8(header_bytes)
        .map_err(|_| Error::UnexpectedResponse("headers are not valid utf-8"))?;

    let mut headers = BTreeMap::new();
    for line in header_str.split("\r\n").filter(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(key.to_owned(), value.to_owned());
    }
    Ok((headers, payload))
}

#[derive(Debug, Deserialize)]
struct MetadataEnvelope {
    #[serde(rename = "Metadata")]
    metadata: Vec<MetadataItem>,
}

#[derive(Debug, Deserialize)]
struct MetadataItem {
    #[serde(rename = "Type")]
    kind: String,
    #[serde(rename = "Data")]
    data: Option<MetadataData>,
}

#[derive(Debug, Deserialize)]
struct MetadataData {
    #[serde(rename = "Offset")]
    offset: u64,
    #[serde(rename = "Duration")]
    duration: u64,
    #[serde(rename = "text")]
    text: MetadataText,
}

#[derive(Debug, Deserialize)]
struct MetadataText {
    #[serde(rename = "Text")]
    text: String,
}

pub fn parse_metadata(payload: &[u8], offset_compensation: u64) -> Result<Vec<SynthesisEvent>> {
    let envelope: MetadataEnvelope = serde_json::from_slice(payload)?;
    let mut events = Vec::new();

    for item in envelope.metadata {
        match item.kind.as_str() {
            "WordBoundary" | "SentenceBoundary" => {
                let data = item
                    .data
                    .ok_or(Error::UnexpectedResponse("boundary metadata missing data"))?;
                let kind = if item.kind == "WordBoundary" {
                    Boundary::Word
                } else {
                    Boundary::Sentence
                };
                events.push(SynthesisEvent::Boundary(BoundaryEvent {
                    kind,
                    offset_ticks: data.offset + offset_compensation,
                    duration_ticks: data.duration,
                    text: unescape_xml(&data.text.text),
                }));
            }
            "SessionEnd" => {}
            other => return Err(Error::UnknownMetadata(other.to_owned())),
        }
    }

    Ok(events)
}

pub fn offset_from_audio_bytes(bytes: usize) -> u64 {
    (bytes as u64 * 8 * TICKS_PER_SECOND) / MP3_BITRATE_BPS
}

fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

pub fn voice_headers() -> [(&'static str, String); 8] {
    [
        ("Authority", "speech.platform.bing.com".to_owned()),
        (
            "Sec-CH-UA",
            format!(
                "\" Not;A Brand\";v=\"99\", \"Microsoft Edge\";v=\"{0}\", \"Chromium\";v=\"{0}\"",
                crate::constants::CHROMIUM_MAJOR_VERSION
            ),
        ),
        ("Sec-CH-UA-Mobile", "?0".to_owned()),
        ("Accept", "*/*".to_owned()),
        ("Sec-Fetch-Site", "none".to_owned()),
        ("Sec-Fetch-Mode", "cors".to_owned()),
        ("Sec-Fetch-Dest", "empty".to_owned()),
        ("User-Agent", crate::constants::user_agent()),
    ]
}

pub fn websocket_headers(muid: &str) -> [(&'static str, String); 8] {
    [
        ("Pragma", "no-cache".to_owned()),
        ("Cache-Control", "no-cache".to_owned()),
        (
            "Origin",
            "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold".to_owned(),
        ),
        ("Sec-WebSocket-Version", "13".to_owned()),
        ("User-Agent", crate::constants::user_agent()),
        ("Accept-Encoding", "gzip, deflate, br, zstd".to_owned()),
        ("Accept-Language", "en-US,en;q=0.9".to_owned()),
        ("Cookie", format!("muid={muid};")),
    ]
}

pub fn sec_ms_gec_version() -> &'static str {
    SEC_MS_GEC_VERSION
}
