use crate::constants::DEFAULT_VOICE;
use crate::error::{Error, Result};
use crate::types::Boundary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeakOptions {
    pub voice: String,
    pub rate: String,
    pub volume: String,
    pub pitch: String,
    pub boundary: Boundary,
}

impl Default for SpeakOptions {
    fn default() -> Self {
        Self {
            voice: DEFAULT_VOICE.to_owned(),
            rate: "+0%".to_owned(),
            volume: "+0%".to_owned(),
            pitch: "+0Hz".to_owned(),
            boundary: Boundary::Sentence,
        }
    }
}

impl SpeakOptions {
    pub fn validate(&self) -> Result<()> {
        let normalized = normalize_voice(&self.voice)?;
        validate_voice(&normalized)?;
        validate_signed(&self.rate, '%').map_err(|_| Error::InvalidRate(self.rate.clone()))?;
        validate_signed(&self.volume, '%')
            .map_err(|_| Error::InvalidVolume(self.volume.clone()))?;
        validate_signed(&self.pitch, 'H').map_err(|_| Error::InvalidPitch(self.pitch.clone()))?;
        if !self.pitch.ends_with("Hz") {
            return Err(Error::InvalidPitch(self.pitch.clone()));
        }
        Ok(())
    }
}

pub fn normalize_voice(input: &str) -> Result<String> {
    if input.starts_with("Microsoft Server Speech Text to Speech Voice (") {
        validate_voice(input)?;
        return Ok(input.to_owned());
    }

    let mut parts = input.splitn(3, '-');
    let language = parts
        .next()
        .ok_or_else(|| Error::InvalidVoice(input.to_owned()))?;
    let region = parts
        .next()
        .ok_or_else(|| Error::InvalidVoice(input.to_owned()))?;
    let mut name = parts
        .next()
        .ok_or_else(|| Error::InvalidVoice(input.to_owned()))?;

    if !language.chars().all(|ch| ch.is_ascii_lowercase()) || language.len() < 2 {
        return Err(Error::InvalidVoice(input.to_owned()));
    }

    if !region.chars().all(|ch| ch.is_ascii_uppercase()) || region.len() != 2 {
        return Err(Error::InvalidVoice(input.to_owned()));
    }

    let mut region_owned = region.to_owned();
    if let Some((prefix, suffix)) = name.split_once('-') {
        region_owned.push('-');
        region_owned.push_str(prefix);
        name = suffix;
    }

    if !name.ends_with("Neural") {
        return Err(Error::InvalidVoice(input.to_owned()));
    }

    Ok(format!(
        "Microsoft Server Speech Text to Speech Voice ({language}-{region_owned}, {name})"
    ))
}

fn validate_voice(value: &str) -> Result<()> {
    if value.starts_with("Microsoft Server Speech Text to Speech Voice (")
        && value.ends_with(')')
        && value.contains(", ")
    {
        Ok(())
    } else {
        Err(Error::InvalidVoice(value.to_owned()))
    }
}

fn validate_signed(value: &str, suffix: char) -> std::result::Result<(), ()> {
    let bytes = value.as_bytes();
    if bytes.len() < 3 {
        return Err(());
    }
    if !matches!(bytes[0], b'+' | b'-') {
        return Err(());
    }
    if suffix == '%' {
        if *bytes.last().unwrap() != b'%' {
            return Err(());
        }
        if !value[1..value.len() - 1]
            .chars()
            .all(|ch| ch.is_ascii_digit())
        {
            return Err(());
        }
        return Ok(());
    }

    if !value.ends_with("Hz") {
        return Err(());
    }
    if !value[1..value.len() - 2]
        .chars()
        .all(|ch| ch.is_ascii_digit())
    {
        return Err(());
    }
    Ok(())
}
