use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use async_stream::try_stream;
use bytes::Bytes;
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
    connect_timeout: Duration,
    receive_timeout: Duration,
    request_chunk_reuse: bool,
    ws_pool: Arc<WsPool>,
}

#[derive(Debug, Clone)]
pub struct EdgeTtsClientBuilder {
    connect_timeout: Duration,
    receive_timeout: Duration,
    ws_pool_size: usize,
    ws_idle_ttl: Duration,
    ws_warmup: bool,
    request_chunk_reuse: bool,
}

#[derive(Debug)]
struct WsPool {
    target_idle: usize,
    idle_ttl: Duration,
    warmup: bool,
    state: Mutex<WsPoolState>,
}

#[derive(Debug, Default)]
struct WsPoolState {
    idle: Vec<IdleWs>,
    warming: usize,
}

#[derive(Debug)]
struct IdleWs {
    stream: WsStream,
    returned_at: Instant,
}

#[derive(Debug)]
struct PooledWebsocket {
    stream: Option<WsStream>,
    reusable: bool,
    pool: Arc<WsPool>,
}

#[derive(Debug)]
struct ChunkFailure {
    err: Error,
    retryable_on_fresh_connection: bool,
}

#[derive(Debug)]
enum ChunkFrame {
    Event(SynthesisEvent),
    Continue,
    TurnEnd,
}

impl Default for EdgeTtsClientBuilder {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            receive_timeout: Duration::from_secs(60),
            ws_pool_size: 1,
            ws_idle_ttl: Duration::from_secs(15),
            ws_warmup: true,
            request_chunk_reuse: true,
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

    pub fn ws_pool_size(mut self, size: usize) -> Self {
        self.ws_pool_size = size;
        self
    }

    pub fn ws_idle_ttl(mut self, ttl: Duration) -> Self {
        self.ws_idle_ttl = ttl;
        self
    }

    pub fn ws_warmup(mut self, enabled: bool) -> Self {
        self.ws_warmup = enabled;
        self
    }

    pub fn request_chunk_reuse(mut self, enabled: bool) -> Self {
        self.request_chunk_reuse = enabled;
        self
    }

    pub fn build(self) -> Result<EdgeTtsClient> {
        let http = Client::builder()
            .connect_timeout(self.connect_timeout)
            .timeout(self.receive_timeout)
            .use_rustls_tls()
            .build()?;
        let client = EdgeTtsClient {
            http,
            connect_timeout: self.connect_timeout,
            receive_timeout: self.receive_timeout,
            request_chunk_reuse: self.request_chunk_reuse,
            ws_pool: Arc::new(WsPool {
                target_idle: self.ws_pool_size,
                idle_ttl: self.ws_idle_ttl,
                warmup: self.ws_warmup,
                state: Mutex::new(WsPoolState::default()),
            }),
        };
        client.ensure_warm_pool();
        Ok(client)
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
            let mut pending_error = None;
            let mut buffered_events = VecDeque::new();

            if client.request_chunk_reuse {
                let mut shared_socket = match client.acquire_websocket().await {
                    Ok(socket) => Some(socket),
                    Err(err) => {
                        pending_error = Some(err);
                        None
                    }
                };

                if let Some(socket) = shared_socket.as_mut() {
                    let mut fallback_at = None;

                    for (index, chunk) in chunks.iter().enumerate() {
                        let offset_compensation = offset_from_audio_bytes(cumulative_audio_bytes);
                        match client.send_chunk_request(socket.stream_mut(), &options, chunk).await {
                            Ok(()) => {
                                loop {
                                    match client
                                        .read_chunk_frame(
                                            socket.stream_mut(),
                                            offset_compensation,
                                            &mut buffered_events,
                                        )
                                        .await
                                    {
                                        Ok(ChunkFrame::Event(event)) => {
                                            if let SynthesisEvent::Audio(chunk) = &event {
                                                cumulative_audio_bytes += chunk.len();
                                                audio_received = true;
                                            }
                                            yield event;
                                        }
                                        Ok(ChunkFrame::Continue) => {}
                                        Ok(ChunkFrame::TurnEnd) => break,
                                        Err(failure) => {
                                            socket.mark_dirty();
                                            if index > 0 && failure.retryable_on_fresh_connection {
                                                fallback_at = Some(index);
                                            } else {
                                                pending_error = Some(failure.err);
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(failure) => {
                                socket.mark_dirty();
                                if index > 0 && failure.retryable_on_fresh_connection {
                                    fallback_at = Some(index);
                                    break;
                                }
                                pending_error = Some(failure.err);
                                break;
                            }
                        }

                        if fallback_at.is_some() || pending_error.is_some() {
                            break;
                        }
                    }

                    if fallback_at.is_some() {
                        if let Some(socket) = shared_socket.as_mut() {
                            socket.mark_dirty();
                        }
                    }

                    drop(shared_socket);

                    if let Some(start_index) = fallback_at {
                        for chunk in chunks.iter().skip(start_index) {
                            let offset_compensation = offset_from_audio_bytes(cumulative_audio_bytes);
                            let mut socket = match client.acquire_websocket().await {
                                Ok(socket) => socket,
                                Err(err) => {
                                    pending_error = Some(err);
                                    break;
                                }
                            };
                            match client.send_chunk_request(socket.stream_mut(), &options, chunk).await {
                                Ok(()) => {
                                    loop {
                                        match client
                                            .read_chunk_frame(
                                                socket.stream_mut(),
                                                offset_compensation,
                                                &mut buffered_events,
                                            )
                                            .await
                                        {
                                            Ok(ChunkFrame::Event(event)) => {
                                                if let SynthesisEvent::Audio(chunk) = &event {
                                                    cumulative_audio_bytes += chunk.len();
                                                    audio_received = true;
                                                }
                                                yield event;
                                            }
                                            Ok(ChunkFrame::Continue) => {}
                                            Ok(ChunkFrame::TurnEnd) => break,
                                            Err(failure) => {
                                                socket.mark_dirty();
                                                pending_error = Some(failure.err);
                                                break;
                                            }
                                        }
                                    }
                                }
                                Err(failure) => {
                                    socket.mark_dirty();
                                    pending_error = Some(failure.err);
                                    break;
                                }
                            }

                            if pending_error.is_some() {
                                break;
                            }
                        }
                    }
                }
            } else {
                for chunk in &chunks {
                    let offset_compensation = offset_from_audio_bytes(cumulative_audio_bytes);
                    let mut socket = match client.acquire_websocket().await {
                        Ok(socket) => socket,
                        Err(err) => {
                            pending_error = Some(err);
                            break;
                        }
                    };
                    match client.send_chunk_request(socket.stream_mut(), &options, chunk).await {
                        Ok(()) => {
                            loop {
                                match client
                                    .read_chunk_frame(
                                        socket.stream_mut(),
                                        offset_compensation,
                                        &mut buffered_events,
                                    )
                                    .await
                                {
                                    Ok(ChunkFrame::Event(event)) => {
                                        if let SynthesisEvent::Audio(chunk) = &event {
                                            cumulative_audio_bytes += chunk.len();
                                            audio_received = true;
                                        }
                                        yield event;
                                    }
                                    Ok(ChunkFrame::Continue) => {}
                                    Ok(ChunkFrame::TurnEnd) => break,
                                    Err(failure) => {
                                        socket.mark_dirty();
                                        pending_error = Some(failure.err);
                                        break;
                                    }
                                }
                            }
                        }
                        Err(failure) => {
                            socket.mark_dirty();
                            pending_error = Some(failure.err);
                            break;
                        }
                    }

                    if pending_error.is_some() {
                        break;
                    }
                }
            }

            if let Some(err) = pending_error {
                Err(err)?;
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

    async fn send_chunk_request(
        &self,
        websocket: &mut WsStream,
        options: &SpeakOptions,
        chunk: &str,
    ) -> std::result::Result<(), ChunkFailure> {
        let config_message = speech_config_message(options.boundary);
        let ssml_message = ssml_message(options, chunk).map_err(|err| ChunkFailure {
            err,
            retryable_on_fresh_connection: false,
        })?;

        debug_frame("send-config", config_message.as_bytes());
        websocket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                config_message.into(),
            ))
            .await
            .map_err(|err| ChunkFailure {
                err: err.into(),
                retryable_on_fresh_connection: true,
            })?;
        debug_frame("send-ssml", ssml_message.as_bytes());
        websocket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                ssml_message.into(),
            ))
            .await
            .map_err(|err| ChunkFailure {
                err: err.into(),
                retryable_on_fresh_connection: true,
            })?;
        Ok(())
    }

    async fn read_chunk_frame(
        &self,
        websocket: &mut WsStream,
        offset_compensation: u64,
        buffered_events: &mut VecDeque<SynthesisEvent>,
    ) -> std::result::Result<ChunkFrame, ChunkFailure> {
        if let Some(event) = buffered_events.pop_front() {
            return Ok(ChunkFrame::Event(event));
        }

        let next = timeout(self.receive_timeout, websocket.next())
            .await
            .map_err(|_| ChunkFailure {
                err: Error::UnexpectedResponse("websocket receive timeout"),
                retryable_on_fresh_connection: false,
            })?;
        let Some(message) = next else {
            return Err(ChunkFailure {
                err: Error::UnexpectedResponse("websocket closed before turn end"),
                retryable_on_fresh_connection: false,
            });
        };

        match message {
            Ok(tokio_tungstenite::tungstenite::Message::Text(text_frame)) => {
                let data = text_frame.as_bytes();
                debug_frame("text", data);
                let header_end = data
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .ok_or(ChunkFailure {
                        err: Error::MissingHeaders,
                        retryable_on_fresh_connection: false,
                    })?;
                let (headers, payload) =
                    parse_headers(data, header_end).map_err(|err| ChunkFailure {
                        err,
                        retryable_on_fresh_connection: false,
                    })?;
                match headers.get("Path").map(String::as_str) {
                    Some("audio.metadata") => {
                        let events =
                            parse_metadata(payload, offset_compensation).map_err(|err| {
                                ChunkFailure {
                                    err,
                                    retryable_on_fresh_connection: false,
                                }
                            })?;
                        if events.is_empty() {
                            Ok(ChunkFrame::Continue)
                        } else {
                            buffered_events.extend(events);
                            Ok(ChunkFrame::Event(
                                buffered_events
                                    .pop_front()
                                    .expect("metadata buffer populated"),
                            ))
                        }
                    }
                    Some("turn.end") => Ok(ChunkFrame::TurnEnd),
                    Some("response") | Some("turn.start") => Ok(ChunkFrame::Continue),
                    Some(other) => Err(ChunkFailure {
                        err: Error::UnknownPath(other.to_owned()),
                        retryable_on_fresh_connection: false,
                    }),
                    None => Err(ChunkFailure {
                        err: Error::MissingHeaders,
                        retryable_on_fresh_connection: false,
                    }),
                }
            }
            Ok(tokio_tungstenite::tungstenite::Message::Binary(frame)) => {
                debug_frame("binary", &frame);
                if frame.len() < 2 {
                    return Err(ChunkFailure {
                        err: Error::UnexpectedResponse("binary frame too short"),
                        retryable_on_fresh_connection: false,
                    });
                }
                let header_length = u16::from_be_bytes([frame[0], frame[1]]) as usize;
                let (headers, payload) =
                    parse_binary_headers(&frame, header_length).map_err(|err| ChunkFailure {
                        err,
                        retryable_on_fresh_connection: false,
                    })?;
                if headers.get("Path").map(String::as_str) != Some("audio") {
                    return Err(ChunkFailure {
                        err: Error::UnexpectedResponse("binary frame path was not audio"),
                        retryable_on_fresh_connection: false,
                    });
                }
                match headers.get("Content-Type").map(String::as_str) {
                    Some("audio/mpeg") => {
                        if payload.is_empty() {
                            return Err(ChunkFailure {
                                err: Error::UnexpectedResponse("audio frame missing payload"),
                                retryable_on_fresh_connection: false,
                            });
                        }
                        Ok(ChunkFrame::Event(SynthesisEvent::Audio(
                            Bytes::copy_from_slice(payload),
                        )))
                    }
                    None if payload.is_empty() => Ok(ChunkFrame::Continue),
                    None => Err(ChunkFailure {
                        err: Error::UnexpectedResponse(
                            "binary frame had payload without content type",
                        ),
                        retryable_on_fresh_connection: false,
                    }),
                    Some(_) => Err(ChunkFailure {
                        err: Error::UnexpectedResponse("unexpected binary content type"),
                        retryable_on_fresh_connection: false,
                    }),
                }
            }
            Ok(tokio_tungstenite::tungstenite::Message::Close(frame)) => {
                if std::env::var_os("EDGE_TTS_DEBUG").is_some() {
                    eprintln!("[edge-tts-debug] close: {frame:?}");
                }
                Err(ChunkFailure {
                    err: Error::UnexpectedResponse("websocket closed before turn end"),
                    retryable_on_fresh_connection: false,
                })
            }
            Ok(
                tokio_tungstenite::tungstenite::Message::Ping(_)
                | tokio_tungstenite::tungstenite::Message::Pong(_)
                | tokio_tungstenite::tungstenite::Message::Frame(_),
            ) => Ok(ChunkFrame::Continue),
            Err(err) => Err(ChunkFailure {
                err: err.into(),
                retryable_on_fresh_connection: false,
            }),
        }
    }

    async fn acquire_websocket(&self) -> Result<PooledWebsocket> {
        if let Some(stream) = self.take_idle_websocket() {
            self.ensure_warm_pool();
            return Ok(PooledWebsocket {
                stream: Some(stream),
                reusable: true,
                pool: Arc::clone(&self.ws_pool),
            });
        }

        let stream = self.connect_websocket_fresh().await?;
        self.ensure_warm_pool();
        Ok(PooledWebsocket {
            stream: Some(stream),
            reusable: true,
            pool: Arc::clone(&self.ws_pool),
        })
    }

    fn take_idle_websocket(&self) -> Option<WsStream> {
        if self.ws_pool.target_idle == 0 {
            return None;
        }

        let mut state = self.ws_pool.state.lock().expect("websocket pool poisoned");
        let now = Instant::now();
        while let Some(idle) = state.idle.pop() {
            if self.ws_pool.is_expired(idle.returned_at, now) {
                continue;
            }
            return Some(idle.stream);
        }
        None
    }

    fn ensure_warm_pool(&self) {
        if !self.ws_pool.warmup || self.ws_pool.target_idle == 0 {
            return;
        }

        let to_spawn = {
            let mut state = self.ws_pool.state.lock().expect("websocket pool poisoned");
            state
                .idle
                .retain(|idle| !self.ws_pool.is_expired(idle.returned_at, Instant::now()));
            let missing = self
                .ws_pool
                .replenishment_needed(state.idle.len(), state.warming);
            state.warming += missing;
            missing
        };

        for _ in 0..to_spawn {
            let client = self.clone();
            tokio::spawn(async move {
                let stream = client.connect_websocket_fresh().await.ok();
                {
                    let mut state = client
                        .ws_pool
                        .state
                        .lock()
                        .expect("websocket pool poisoned");
                    state.warming = state.warming.saturating_sub(1);
                    if let Some(stream) = stream {
                        if state.idle.len() < client.ws_pool.target_idle {
                            state.idle.push(IdleWs {
                                stream,
                                returned_at: Instant::now(),
                            });
                        }
                    }
                }
                client.ensure_warm_pool();
            });
        }
    }

    async fn connect_websocket_fresh(&self) -> Result<WsStream> {
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

        let connect = timeout(self.connect_timeout, connect_async(request))
            .await
            .map_err(|_| Error::UnexpectedResponse("websocket connect timeout"))?;
        let (stream, _) = connect?;
        Ok(stream)
    }
}

impl PooledWebsocket {
    fn stream_mut(&mut self) -> &mut WsStream {
        self.stream
            .as_mut()
            .expect("pooled websocket missing stream")
    }

    fn mark_dirty(&mut self) {
        self.reusable = false;
    }
}

impl Drop for PooledWebsocket {
    fn drop(&mut self) {
        let Some(stream) = self.stream.take() else {
            return;
        };
        if !self.reusable || self.pool.target_idle == 0 {
            return;
        }

        let mut state = self.pool.state.lock().expect("websocket pool poisoned");
        if state.idle.len() < self.pool.target_idle {
            state.idle.push(IdleWs {
                stream,
                returned_at: Instant::now(),
            });
        }
    }
}

impl WsPool {
    fn is_expired(&self, returned_at: Instant, now: Instant) -> bool {
        now.saturating_duration_since(returned_at) >= self.idle_ttl
    }

    fn replenishment_needed(&self, idle_len: usize, warming: usize) -> usize {
        self.target_idle
            .saturating_sub(idle_len.saturating_add(warming))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_defaults_enable_pooling_and_chunk_reuse() {
        let builder = EdgeTtsClientBuilder::default();
        assert_eq!(builder.ws_pool_size, 1);
        assert_eq!(builder.ws_idle_ttl, Duration::from_secs(15));
        assert!(builder.ws_warmup);
        assert!(builder.request_chunk_reuse);
    }

    #[test]
    fn pool_replenishment_respects_idle_and_warming_counts() {
        let pool = WsPool {
            target_idle: 2,
            idle_ttl: Duration::from_secs(15),
            warmup: true,
            state: Mutex::new(WsPoolState::default()),
        };

        assert_eq!(pool.replenishment_needed(0, 0), 2);
        assert_eq!(pool.replenishment_needed(1, 0), 1);
        assert_eq!(pool.replenishment_needed(1, 1), 0);
        assert_eq!(pool.replenishment_needed(2, 0), 0);
    }

    #[test]
    fn idle_connection_ttl_only_applies_after_expiration() {
        let pool = WsPool {
            target_idle: 1,
            idle_ttl: Duration::from_secs(15),
            warmup: true,
            state: Mutex::new(WsPoolState::default()),
        };
        let now = Instant::now();

        assert!(!pool.is_expired(now - Duration::from_secs(14), now));
        assert!(pool.is_expired(now - Duration::from_secs(15), now));
    }
}
