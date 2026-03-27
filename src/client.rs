use std::pin::Pin;
use std::time::{Duration, SystemTime};

use async_stream::try_stream;
use futures_util::{SinkExt, Stream, StreamExt};
use reqwest::Client;
use tokio::fs;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use url::Url;

use crate::constants::{TEXT_CHUNK_LIMIT, voice_list_url, websocket_url};
use crate::error::{Error, Result};
use crate::options::SpeakOptions;
use crate::protocol::{
    generate_connection_id, generate_muid, generate_sec_ms_gec, offset_from_audio_bytes,
    parse_binary_headers, parse_headers, parse_metadata, sec_ms_gec_version, speech_config_message,
    split_text, ssml_message, voice_headers, websocket_headers,
};
use crate::subtitles::{filter_boundaries, to_srt};
use crate::types::{BoundaryEvent, SynthesisEvent, SynthesisResult, Voice};

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub type EventStream = Pin<Box<dyn Stream<Item = Result<SynthesisEvent>> + Send + Sync + 'static>>;

#[derive(Debug, Clone)]
pub struct EdgeTtsClient {
    http: Client,
    receive_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct EdgeTtsClientBuilder {
    connect_timeout: Duration,
    receive_timeout: Duration,
}

impl Default for EdgeTtsClientBuilder {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            receive_timeout: Duration::from_secs(60),
        }
    }
}

impl EdgeTtsClientBuilder {
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn receive_timeout(mut self, timeout: Duration) -> Self {
        self.receive_timeout = timeout;
        self
    }

    pub fn build(self) -> Result<EdgeTtsClient> {
        let http = Client::builder()
            .connect_timeout(self.connect_timeout)
            .timeout(self.receive_timeout)
            .use_rustls_tls()
            .build()?;
        Ok(EdgeTtsClient {
            http,
            receive_timeout: self.receive_timeout,
        })
    }
}

impl EdgeTtsClient {
    pub fn builder() -> EdgeTtsClientBuilder {
        EdgeTtsClientBuilder::default()
    }

    pub fn new() -> Result<Self> {
        Self::builder().build()
    }

    pub async fn list_voices(&self) -> Result<Vec<Voice>> {
        let sec_ms_gec = generate_sec_ms_gec(SystemTime::now());
        let muid = generate_muid();
        let mut request = self
            .http
            .get(format!(
                "{}&Sec-MS-GEC={sec_ms_gec}&Sec-MS-GEC-Version={}",
                voice_list_url(),
                sec_ms_gec_version()
            ))
            .header("Cookie", format!("muid={muid};"))
            .header("Accept-Encoding", "gzip, deflate, br, zstd")
            .header("Accept-Language", "en-US,en;q=0.9");

        for (name, value) in voice_headers() {
            request = request.header(name, value);
        }

        Ok(request.send().await?.error_for_status()?.json().await?)
    }

    pub async fn stream(
        &self,
        text: impl Into<String>,
        options: SpeakOptions,
    ) -> Result<EventStream> {
        options.validate()?;
        let text = text.into();
        let chunks = split_text(&text, TEXT_CHUNK_LIMIT)?;
        let client = self.clone();

        Ok(Box::pin(try_stream! {
            let mut cumulative_audio_bytes = 0usize;
            let mut audio_received = false;

            for chunk in chunks {
                let mut chunk_audio_bytes = 0usize;
                let offset_compensation = offset_from_audio_bytes(cumulative_audio_bytes);
                let mut websocket = client.connect_websocket().await?;
                let config_message = speech_config_message(options.boundary);
                let ssml_message = ssml_message(&options, &chunk)?;

                debug_frame("send-config", config_message.as_bytes());
                websocket
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        config_message.into(),
                    ))
                    .await?;
                debug_frame("send-ssml", ssml_message.as_bytes());
                websocket
                    .send(tokio_tungstenite::tungstenite::Message::Text(ssml_message.into()))
                    .await?;

                loop {
                    let next = timeout(client.receive_timeout, websocket.next()).await
                        .map_err(|_| Error::UnexpectedResponse("websocket receive timeout"))?;
                    let Some(message) = next else {
                        break;
                    };
                    match message? {
                        tokio_tungstenite::tungstenite::Message::Text(text_frame) => {
                            let data = text_frame.as_bytes();
                            debug_frame("text", data);
                            let header_end = data
                                .windows(4)
                                .position(|window| window == b"\r\n\r\n")
                                .ok_or(Error::MissingHeaders)?;
                            let (headers, payload) = parse_headers(data, header_end)?;
                            match headers.get("Path").map(String::as_str) {
                                Some("audio.metadata") => {
                                    for event in parse_metadata(payload, offset_compensation)? {
                                        yield event;
                                    }
                                }
                                Some("turn.end") => break,
                                Some("response") | Some("turn.start") => {}
                                Some(other) => Err(Error::UnknownPath(other.to_owned()))?,
                                None => Err(Error::MissingHeaders)?,
                            }
                        }
                        tokio_tungstenite::tungstenite::Message::Binary(frame) => {
                            debug_frame("binary", &frame);
                            if frame.len() < 2 {
                                Err(Error::UnexpectedResponse("binary frame too short"))?;
                            }
                            let header_length = u16::from_be_bytes([frame[0], frame[1]]) as usize;
                            let (headers, payload) = parse_binary_headers(&frame, header_length)?;
                            if headers.get("Path").map(String::as_str) != Some("audio") {
                                Err(Error::UnexpectedResponse("binary frame path was not audio"))?;
                            }
                            match headers.get("Content-Type").map(String::as_str) {
                                Some("audio/mpeg") => {
                                    if payload.is_empty() {
                                        Err(Error::UnexpectedResponse("audio frame missing payload"))?;
                                    }
                                    chunk_audio_bytes += payload.len();
                                    audio_received = true;
                                    yield SynthesisEvent::Audio(bytes::Bytes::copy_from_slice(payload));
                                }
                                None if payload.is_empty() => {}
                                None => Err(Error::UnexpectedResponse("binary frame had payload without content type"))?,
                                Some(_) => Err(Error::UnexpectedResponse("unexpected binary content type"))?,
                            }
                        }
                        tokio_tungstenite::tungstenite::Message::Close(frame) => {
                            if std::env::var_os("EDGE_TTS_DEBUG").is_some() {
                                eprintln!("[edge-tts-debug] close: {frame:?}");
                            }
                            break
                        }
                        tokio_tungstenite::tungstenite::Message::Ping(_)
                        | tokio_tungstenite::tungstenite::Message::Pong(_)
                        | tokio_tungstenite::tungstenite::Message::Frame(_) => {}
                    }
                }

                cumulative_audio_bytes += chunk_audio_bytes;
            }

            if !audio_received {
                Err(Error::NoAudioReceived)?;
            }
        }))
    }

    pub async fn synthesize(
        &self,
        text: impl Into<String>,
        options: SpeakOptions,
    ) -> Result<SynthesisResult> {
        let mut stream = self.stream(text, options).await?;
        let mut audio = Vec::new();
        let mut boundaries = Vec::new();

        while let Some(event) = stream.next().await {
            match event? {
                SynthesisEvent::Audio(chunk) => audio.extend_from_slice(&chunk),
                SynthesisEvent::Boundary(boundary) => boundaries.push(boundary),
            }
        }

        Ok(SynthesisResult { audio, boundaries })
    }

    pub async fn save(
        &self,
        text: impl Into<String>,
        options: SpeakOptions,
        audio_path: impl AsRef<std::path::Path>,
        srt_path: Option<impl AsRef<std::path::Path>>,
    ) -> Result<SynthesisResult> {
        let result = self.synthesize(text, options.clone()).await?;
        fs::write(audio_path, &result.audio).await?;
        if let Some(path) = srt_path {
            let filtered = filter_boundaries(&result.boundaries, options.boundary);
            fs::write(path, to_srt(&filtered)).await?;
        }
        Ok(result)
    }

    async fn connect_websocket(&self) -> Result<WsStream> {
        let sec_ms_gec = generate_sec_ms_gec(SystemTime::now());
        let muid = generate_muid();
        let url = Url::parse(&format!(
            "{}&ConnectionId={}&Sec-MS-GEC={sec_ms_gec}&Sec-MS-GEC-Version={}",
            websocket_url(),
            generate_connection_id(),
            sec_ms_gec_version(),
        ))
        .map_err(|_| Error::UnexpectedResponse("invalid websocket url"))?;

        let mut request = url.as_str().into_client_request()?;
        for (name, value) in websocket_headers(&muid) {
            request.headers_mut().insert(
                http::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| Error::UnexpectedResponse("invalid header name"))?,
                http::HeaderValue::from_str(&value)
                    .map_err(|_| Error::UnexpectedResponse("invalid header value"))?,
            );
        }
        let (stream, _) = connect_async(request).await?;
        Ok(stream)
    }
}

pub fn subtitles(events: &[BoundaryEvent]) -> String {
    to_srt(events)
}

fn debug_frame(kind: &str, payload: &[u8]) {
    if std::env::var_os("EDGE_TTS_DEBUG").is_some() {
        eprintln!(
            "[edge-tts-debug] {kind}: {}",
            String::from_utf8_lossy(payload)
        );
    }
}
