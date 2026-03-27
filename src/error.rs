use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid rate value: {0}")]
    InvalidRate(String),
    #[error("invalid volume value: {0}")]
    InvalidVolume(String),
    #[error("invalid pitch value: {0}")]
    InvalidPitch(String),
    #[error("invalid voice value: {0}")]
    InvalidVoice(String),
    #[error("text chunk size must be greater than zero")]
    InvalidChunkSize,
    #[error("failed to split text safely")]
    InvalidSplitPoint,
    #[error("websocket response was missing expected headers")]
    MissingHeaders,
    #[error("unexpected websocket response: {0}")]
    UnexpectedResponse(&'static str),
    #[error("unknown websocket path: {0}")]
    UnknownPath(String),
    #[error("unknown metadata type: {0}")]
    UnknownMetadata(String),
    #[error("no audio was received from the service")]
    NoAudioReceived,
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("websocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http request build error: {0}")]
    HttpRequest(#[from] http::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
