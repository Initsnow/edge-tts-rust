pub const BASE_URL: &str = "speech.platform.bing.com/consumer/speech/synthesize/readaloud";
pub const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
pub const DEFAULT_VOICE: &str = "en-US-EmmaMultilingualNeural";
pub const OUTPUT_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";
pub const TEXT_CHUNK_LIMIT: usize = 4096;
pub const TICKS_PER_SECOND: u64 = 10_000_000;
pub const MP3_BITRATE_BPS: u64 = 48_000;
pub const CHROMIUM_MAJOR_VERSION: &str = "143";
pub const SEC_MS_GEC_VERSION: &str = "1-143.0.3650.75";

pub fn websocket_url() -> String {
    format!("wss://{BASE_URL}/edge/v1?TrustedClientToken={TRUSTED_CLIENT_TOKEN}")
}

pub fn voice_list_url() -> String {
    format!("https://{BASE_URL}/voices/list?trustedclienttoken={TRUSTED_CLIENT_TOKEN}")
}

pub fn user_agent() -> String {
    format!(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/{CHROMIUM_MAJOR_VERSION}.0.0.0 Safari/537.36 \
         Edg/{CHROMIUM_MAJOR_VERSION}.0.0.0"
    )
}
