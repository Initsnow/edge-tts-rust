use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::constants::{
    MP3_BITRATE_BPS, SEC_MS_GEC_VERSION, TICKS_PER_SECOND, TRUSTED_CLIENT_TOKEN,
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
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(32);
    for byte in bytes {
        s.push(HEX[(byte >> 4) as usize] as char);
        s.push(HEX[(byte & 0xf) as usize] as char);
    }
    s
}

const COPYRIGHT_PREFIX: &[u8] = b"Copyright (C) Microsoft Corporation. All rights reserved.";

const CS64_COEFFICIENTS: [(u32, u32); 14] = [
    (0xE82E91B1, 0xB32700D0),
    (0x44DE24C6, 0x88DFF51D),
    (0x3BEB65BC, 0x29514038),
    (0xC89C5933, 0xB328CBDF),
    (0x8D1EC211, 0xD405B4D5),
    (0x314EF5D5, 0x26E55F3C),
    (0xE92491B1, 0x607801C6),
    (0xF0A5B9D5, 0x82B2C1FB),
    (0x06064CF3, 0xFA51A3DA),
    (0xDA79FA60, 0x207CC464),
    (0xBED20560, 0x3C3C74F3),
    (0x89F9C9F8, 0xD2D14B61),
    (0x35FA38C6, 0xBB04A2C1),
    (0xCB49E3C6, 0xC5626167),
];

const CS64_FACTOR: u32 = 0x82893933;
const CS64_ADJUSTMENT0: u32 = 0xF2F90B1A;
const CS64_ADJUSTMENT1: u32 = 0x0B1A0000;

#[inline]
fn rol16(v: u32) -> u32 {
    v.rotate_left(16)
}

fn words_le4_even(data: &[u8]) -> Vec<u32> {
    if data.is_empty() {
        return Vec::new();
    }
    let pad_len = (4 - (data.len() % 4)) % 4;
    let full_len = data.len() - (data.len() % 4);
    let mut words = Vec::with_capacity((data.len() + pad_len) / 4 + 1);
    for chunk in data[..full_len].chunks_exact(4) {
        words.push(u32::from_le_bytes(chunk.try_into().unwrap()));
    }
    if pad_len > 0 {
        let mut tail = [0u8; 4];
        tail[..data.len() - full_len].copy_from_slice(&data[full_len..]);
        words.push(u32::from_le_bytes(tail));
    }
    if words.len() % 2 != 0 {
        words.push(0);
    }
    words
}

fn cs64_digest(data: &[u8], digest: &[u8; 32]) -> (u32, u32) {
    let key_a = u32::from_le_bytes(digest[0..4].try_into().unwrap()) | 1;
    let key_b = u32::from_le_bytes(digest[4..8].try_into().unwrap()) | 1;

    let factors = [
        key_a.wrapping_mul(CS64_FACTOR),
        key_b.wrapping_mul(CS64_FACTOR),
    ];

    let mut terms = Vec::with_capacity(14 + (data.len() + 8) / 4);
    let mut offsets = Vec::with_capacity(14 + (data.len() + 8) / 4);

    for (index, &(coefficient, offset)) in CS64_COEFFICIENTS.iter().enumerate() {
        terms.push(coefficient.wrapping_mul(factors[index % 2]));
        offsets.push(offset);
    }

    let mut prepended = Vec::with_capacity(1 + data.len());
    prepended.push(0u8);
    prepended.extend_from_slice(data);

    for (index, word) in words_le4_even(&prepended).into_iter().enumerate() {
        let key = if index % 2 == 0 { key_a } else { key_b };
        let mut term = word.wrapping_mul(key);
        let mul = if index % 2 == 0 { 0xEF0569FB } else { 0xC31713DB };
        let mut offset = rol16(word).wrapping_mul(mul);

        if index == 0 {
            term = term.wrapping_add(CS64_ADJUSTMENT0.wrapping_mul(factors[0]));
            offset = offset.wrapping_add(CS64_ADJUSTMENT1);
        }

        terms.push(term);
        offsets.push(offset);
    }

    let mut swap_acc: u32 = 0;
    let mut swap_last: u32 = 0;
    let mut state: u32 = 0;
    let mut acc: u32 = 0;

    for i in (0..terms.len()).step_by(2) {
        let x = terms[i];
        let y = terms[i + 1];

        let t = rol16(offsets[i].wrapping_add(x));
        let q = t.wrapping_mul(0x76F50F34);
        swap_acc = swap_acc.wrapping_add(rol16(q).wrapping_mul(0xC3EFEA97).wrapping_add(q));

        let t = rol16(offsets[i + 1].wrapping_add(y));
        let q = t.wrapping_mul(0x1881CE3C);
        swap_last = rol16(q).wrapping_mul(0x35BD1EC9).wrapping_add(q);
        swap_acc = swap_acc.wrapping_add(swap_last);

        state = state.wrapping_mul(key_a).wrapping_add(x);
        state = rol16(state).wrapping_mul(0xCF98B111);
        state = rol16(state).wrapping_mul(0x87085B9F);
        state = rol16(state).wrapping_mul(0x12CEB96D);
        state = rol16(state).wrapping_mul(0x257E1D83);
        acc = acc.wrapping_add(state);

        let t = state.wrapping_mul(key_b).wrapping_add(y);
        let t = rol16(t).wrapping_mul(0xA27416F5);
        let t = rol16(t).wrapping_mul(0xD38396FF);
        state = rol16(rol16(t).wrapping_mul(0x7C932B89)).wrapping_mul(0xBFA49F69);
        acc = acc.wrapping_add(state);
    }

    (swap_last ^ state, swap_acc ^ acc)
}

fn bytes_to_hex_upper(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

pub fn generate_sec_ms_gec_v2_from_ticks(ticks: u64, token: &str) -> String {
    let str_to_hash = format!("{ticks}{token}");
    let mut hasher = Sha256::new();
    hasher.update(COPYRIGHT_PREFIX);
    hasher.update(str_to_hash.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();

    let (w0, w1) = cs64_digest(str_to_hash.as_bytes(), &digest);
    let mut cs64_bytes = [0u8; 8];
    cs64_bytes[0..4].copy_from_slice(&w0.to_le_bytes());
    cs64_bytes[4..8].copy_from_slice(&w1.to_le_bytes());

    let cs64_hex = bytes_to_hex_upper(&cs64_bytes);
    let sha256_hex = bytes_to_hex_upper(&digest);
    format!("{cs64_hex}-{sha256_hex}")
}

pub fn generate_sec_ms_gec(now: SystemTime) -> String {
    let unix_seconds = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let rounded = (unix_seconds + WINDOWS_EPOCH_OFFSET_SECONDS) / 300 * 300;
    let windows_ticks = rounded * 10_000_000;
    generate_sec_ms_gec_v2_from_ticks(windows_ticks, TRUSTED_CLIENT_TOKEN)
}

pub fn javascript_timestamp() -> String {
    Utc::now()
        .format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)")
        .to_string()
}

pub fn speech_config_message(boundary: Boundary, output_format: &str) -> String {
    let (word, sentence) = boundary.metadata_flags();
    let payload = json!({
        "context": {
            "synthesis": {
                "audio": {
                    "metadataoptions": {
                        "sentenceBoundaryEnabled": sentence,
                        "wordBoundaryEnabled": word
                    },
                    "outputFormat": output_format
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
    let bytes = text.as_bytes();
    let mut result = String::with_capacity(bytes.len() + bytes.len() / 8);
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let replacement = match b {
            0..=8 | 11..=12 | 14..=31 => Some(" "),
            b'<' => Some("&lt;"),
            b'>' => Some("&gt;"),
            b'"' => Some("&quot;"),
            b'\'' => Some("&apos;"),
            b'&' => {
                let rest = &bytes[i + 1..];
                if rest.starts_with(b"amp;")
                    || rest.starts_with(b"lt;")
                    || rest.starts_with(b"gt;")
                    || rest.starts_with(b"quot;")
                    || rest.starts_with(b"apos;")
                {
                    None
                } else {
                    Some("&amp;")
                }
            }
            _ => None,
        };

        if let Some(rep) = replacement {
            if i > last {
                result.push_str(&text[last..i]);
            }
            result.push_str(rep);
            i += 1;
            last = i;
        } else {
            i += 1;
        }
    }

    if last == 0 {
        return text.to_owned();
    }
    if last < bytes.len() {
        result.push_str(&text[last..]);
    }
    result
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
    let mut events = Vec::with_capacity(envelope.metadata.len());

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

pub fn cbr_bitrate_bps(output_format: &str) -> Option<u64> {
    for part in output_format.split('-') {
        if let Some(val) = part.strip_suffix("kbitrate").and_then(|k| k.parse::<u64>().ok()) {
            return Some(val * 1000);
        }
        if let Some(val) = part.strip_suffix("kbps").and_then(|k| k.parse::<u64>().ok()) {
            return Some(val * 1000);
        }
    }
    None
}

pub fn offset_from_audio_bytes(bytes: usize) -> u64 {
    offset_from_audio_bytes_with_bitrate(bytes, MP3_BITRATE_BPS)
}

pub fn offset_from_audio_bytes_with_bitrate(bytes: usize, bitrate_bps: u64) -> u64 {
    if bitrate_bps == 0 {
        0
    } else {
        (bytes as u64 * 8 * TICKS_PER_SECOND) / bitrate_bps
    }
}

fn unescape_xml(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
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
