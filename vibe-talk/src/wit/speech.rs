//! Adapts the Wit.ai audio client to the shared read-aloud interface.
//!
//! The politeness and honesty rules of [`crate::wit`] are enforced here,
//! around whichever client is plugged in: sentence-sized requests, at
//! most two at a time, a small cache, and a silence check on the merged
//! audio before anything is handed to the player (`#546
//! read-aloud-silent-audio`: audio that says nothing must stop the read,
//! never play as if it had spoken).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, Semaphore};

use super::{sentence_chunks, speed_percent, WitClient, WitError};
use crate::config::WitConfig;
use crate::speech::{Description, Playback, Speech, SpeechError, SpeechProvider, SpeechStream};

/// How many synthesize calls may be in flight at once.
///
/// Two, not one, so the page's prefetch of the next block can overlap the
/// tail of the current one; not more, because Wit.ai rate-limits per app
/// and a read is a queue of sentences, not a batch job.
pub const MAX_IN_FLIGHT: usize = 2;

/// How many synthesized sentence results the cache holds.
///
/// A read re-tap, a scrub back, and the prefetch-then-play of the same
/// block all ask for the same sentence again; 128 sentences is a few
/// messages' worth, which is the reuse there is.
pub const CACHE_ENTRIES: usize = 128;

type CacheKey = (String, String, Option<u32>);

/// Wit.ai read-aloud with its configuration held behind the provider boundary.
pub struct WitSpeech {
    client: Arc<dyn WitClient>,
    config: WitConfig,
    cache: Mutex<HashMap<CacheKey, Vec<u8>>>,
    cache_order: Mutex<VecDeque<CacheKey>>,
    in_flight: Semaphore,
}

impl WitSpeech {
    /// Use the given client's connection pool and the deployment's Wit.ai settings.
    #[must_use]
    pub fn new(client: Arc<dyn WitClient>, config: WitConfig) -> Self {
        Self {
            client,
            config,
            cache: Mutex::new(HashMap::new()),
            cache_order: Mutex::new(VecDeque::new()),
            in_flight: Semaphore::new(MAX_IN_FLIGHT),
        }
    }

    fn token(&self) -> Result<&crate::config::Secret, SpeechError> {
        self.config
            .token
            .as_ref()
            .filter(|token| !token.expose().trim().is_empty())
            .ok_or_else(|| SpeechError::NotConfigured {
                code: "wit_not_configured",
                detail: "wit.token_file is not configured; this server cannot read messages \
                         aloud with Wit.ai"
                    .to_owned(),
            })
    }

    fn voice(&self, chosen: Option<&str>) -> Result<String, SpeechError> {
        chosen
            .map(str::trim)
            .filter(|voice| !voice.is_empty())
            .or_else(|| {
                self.config
                    .voice
                    .as_deref()
                    .map(str::trim)
                    .filter(|voice| !voice.is_empty())
            })
            .map(str::to_owned)
            .ok_or_else(|| SpeechError::NotConfigured {
                code: "wit_not_configured",
                detail: "wit.voice is not configured and no voice was chosen in Settings; \
                         name one in the configuration, for example voice = \"wit$Remi\""
                    .to_owned(),
            })
    }

    async fn cached(&self, key: &CacheKey) -> Option<Vec<u8>> {
        self.cache.lock().await.get(key).cloned()
    }

    async fn store(&self, key: CacheKey, audio: Vec<u8>) {
        let mut cache = self.cache.lock().await;
        let mut order = self.cache_order.lock().await;
        if !cache.contains_key(&key) {
            order.push_back(key.clone());
            while order.len() > CACHE_ENTRIES {
                if let Some(oldest) = order.pop_front() {
                    cache.remove(&oldest);
                }
            }
        }
        cache.insert(key, audio);
    }

    async fn speak_with(
        &self,
        text: &str,
        speed: Option<f64>,
        chosen_voice: Option<&str>,
    ) -> Result<Speech, SpeechError> {
        if text.trim().is_empty() {
            return Err(SpeechError::Empty);
        }
        let token = self.token()?;
        let voice = self.voice(chosen_voice)?;
        let pace = speed_percent(speed.or(self.config.speed));
        let mut pieces = Vec::new();
        for chunk in sentence_chunks(text) {
            let key: CacheKey = (chunk.clone(), voice.clone(), pace);
            if let Some(audio) = self.cached(&key).await {
                pieces.push(audio);
                continue;
            }
            let _permit = self
                .in_flight
                .acquire()
                .await
                .map_err(|_| backend("wit_error", "the Wit.ai request queue closed"))?;
            let audio = self
                .client
                .synthesize(token, &chunk, &voice, pace)
                .await
                .map_err(shared_error)?;
            // Validate BEFORE the cache: a malformed or silent answer is
            // a failure of this read, never a result a re-tap replays.
            // Each piece is checked on its own, so one silent sentence in
            // a spoken message stops the read rather than playing as a
            // gap the reader cannot tell from a pause.
            parse_wav(&audio).map_err(shared_shape)?;
            if is_silent(&audio) {
                return Err(backend(
                    "wit_silent_audio",
                    "Wit.ai returned only silence for this message; nothing was played",
                ));
            }
            self.store(key, audio.clone()).await;
            pieces.push(audio);
        }
        let audio = merge_wavs(&pieces).map_err(shared_shape)?;
        if is_silent(&audio) {
            return Err(backend(
                "wit_silent_audio",
                "Wit.ai returned only silence for this message; nothing was played",
            ));
        }
        Ok(Speech {
            audio,
            content_type: super::AUDIO_CONTENT_TYPE.to_owned(),
        })
    }
}

fn backend(code: &'static str, detail: &str) -> SpeechError {
    SpeechError::Backend {
        code,
        detail: detail.to_owned(),
    }
}

fn shared_error(error: WitError) -> SpeechError {
    let code = error.code();
    let detail = match &error {
        WitError::RateLimited { attempts, .. } => format!(
            "Wit.ai is rate-limiting this app after {attempts} attempts; wait a minute and \
             try reading again"
        ),
        other => other.to_string(),
    };
    SpeechError::Backend { code, detail }
}

fn shared_shape(error: WitError) -> SpeechError {
    shared_error(error)
}

/// The PCM facts one WAV carries, and where its samples sit.
struct Wav {
    channels: u16,
    sample_rate: u32,
    data_start: usize,
    data_len: usize,
}

/// Whether `bytes` parse as one playable WAV piece, for adapters sharing
/// this module's merging and silence rules (`crate::local_tts`).
///
/// # Errors
///
/// [`WitError::Shape`] when the bytes are not 16-bit PCM WAV with a
/// playable rate and channel count.
pub fn parse_wav_for_merge(bytes: &[u8]) -> Result<(), WitError> {
    parse_wav(bytes).map(|_| ())
}

fn parse_wav(bytes: &[u8]) -> Result<Wav, WitError> {
    let shape = |detail: &str| WitError::Shape(detail.to_owned());
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(shape("the answer is not WAV audio"));
    }
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<(usize, usize)> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let kind = &bytes[at..at + 4];
        let len = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        let start = at + 8;
        let end = start.saturating_add(len).min(bytes.len());
        match kind {
            b"fmt " if end - start >= 16 => {
                format = Some((
                    u16::from_le_bytes([bytes[start], bytes[start + 1]]),
                    u16::from_le_bytes([bytes[start + 2], bytes[start + 3]]),
                    u32::from_le_bytes([
                        bytes[start + 4],
                        bytes[start + 5],
                        bytes[start + 6],
                        bytes[start + 7],
                    ]),
                    u16::from_le_bytes([bytes[start + 14], bytes[start + 15]]),
                ));
            }
            b"data" => data = Some((start, end - start)),
            _ => {}
        }
        at = start + len + (len % 2);
    }
    let (audio_format, channels, sample_rate, bits) =
        format.ok_or_else(|| shape("the WAV answer has no format"))?;
    if audio_format != 1 || bits != 16 {
        return Err(shape("the WAV answer is not 16-bit PCM"));
    }
    // A header the browser cannot play is not audio this server may hand
    // back as success: no channels or no rate is silence by another name.
    if channels == 0 || !(1..=384_000).contains(&sample_rate) {
        return Err(shape("the WAV answer has no playable rate or channels"));
    }
    let (data_start, data_len) = data.ok_or_else(|| shape("the WAV answer has no audio"))?;
    Ok(Wav {
        channels,
        sample_rate,
        data_start,
        data_len,
    })
}

/// Merge sentence WAVs into one WAV: the first piece's header, every
/// piece's samples. All pieces must share one format — same voice, same
/// settings — and a mismatch is a vendor answer this server cannot use,
/// never a guess at resampling.
///
/// # Errors
///
/// [`WitError::Shape`] when there are no pieces, a piece is not 16-bit
/// PCM WAV, or the pieces disagree on format.
pub fn merge_wavs(pieces: &[Vec<u8>]) -> Result<Vec<u8>, WitError> {
    let first = pieces
        .first()
        .ok_or_else(|| WitError::Shape("no audio was synthesized".to_owned()))?;
    if pieces.len() == 1 {
        parse_wav(first)?;
        return Ok(first.clone());
    }
    let head = parse_wav(first)?;
    let total: usize = pieces
        .iter()
        .map(|piece| Ok(parse_wav(piece)?.data_len))
        .collect::<Result<Vec<_>, WitError>>()?
        .into_iter()
        .sum();
    for piece in pieces {
        let wav = parse_wav(piece)?;
        if wav.channels != head.channels || wav.sample_rate != head.sample_rate {
            return Err(WitError::Shape(
                "Wit.ai answered sentences in different audio formats".to_owned(),
            ));
        }
    }
    let mut out = Vec::with_capacity(44 + total);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36_u32.saturating_add(total as u32)).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0");
    out.extend_from_slice(&head.channels.to_le_bytes());
    out.extend_from_slice(&head.sample_rate.to_le_bytes());
    out.extend_from_slice(
        &head
            .sample_rate
            .saturating_mul(u32::from(head.channels))
            .saturating_mul(2)
            .to_le_bytes(),
    );
    out.extend_from_slice(&(head.channels * 2).to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(total as u32).to_le_bytes());
    for piece in pieces {
        let wav = parse_wav(piece)?;
        out.extend_from_slice(&piece[wav.data_start..wav.data_start + wav.data_len]);
    }
    Ok(out)
}

/// Whether WAV audio says nothing at all: no sample, or every sample zero.
///
/// A loudness threshold would need tuning against real voices; exact
/// silence needs none, and it is the failure `#546` measured — a vendor
/// refusal answered with frames of zeros.
#[must_use]
pub fn is_silent(wav: &[u8]) -> bool {
    let Ok(parsed) = parse_wav(wav) else {
        return false;
    };
    let samples = &wav[parsed.data_start..parsed.data_start + parsed.data_len];
    samples.is_empty()
        || samples
            .chunks_exact(2)
            .all(|pair| i16::from_le_bytes([pair[0], pair[1]]) == 0)
}

#[async_trait]
impl SpeechProvider for WitSpeech {
    fn describe(&self) -> Description {
        Description {
            backend: "wit",
            label: "Wit.ai voice".to_owned(),
            playback: Playback::Audio,
            local_only: false,
            chain: Vec::new(),
        }
    }

    async fn warm_up(&self) -> Result<(), SpeechError> {
        // Listing the voices opens the reusable connection and proves the
        // token before the reader taps, without synthesizing anything.
        let token = self.token()?;
        self.client.voices(token).await.map_err(shared_error)?;
        Ok(())
    }

    async fn speak(&self, text: &str, speed: Option<f64>) -> Result<Speech, SpeechError> {
        self.speak_with(text, speed, None).await
    }

    async fn speak_stream_voiced(
        &self,
        text: &str,
        speed: Option<f64>,
        _selection: Option<&str>,
        voice: Option<&str>,
    ) -> Result<SpeechStream, SpeechError> {
        let spoken = self.speak_with(text, speed, voice).await?;
        Ok(SpeechStream {
            content_type: spoken.content_type,
            preamble: None,
            voice: None,
            session: None,
            chunks: Box::pin(futures_util::stream::once(async move {
                Ok(bytes::Bytes::from(spoken.audio))
            })),
        })
    }

    async fn voices(&self) -> Result<Vec<crate::speech::VoiceInfo>, SpeechError> {
        let token = self.token()?;
        let voices = self.client.voices(token).await.map_err(shared_error)?;
        Ok(voices
            .into_iter()
            .map(|voice| crate::speech::VoiceInfo {
                name: voice.name,
                locale: voice.locale,
                gender: voice.gender,
                styles: voice.styles,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Secret;
    use crate::wit::fake::{fake_wav, FakeWit};

    fn config() -> WitConfig {
        WitConfig {
            token: Some(Secret::new(crate::wit::fake::VALID_TOKEN)),
            voice: Some("wit$Remi".to_owned()),
            speed: None,
            api_base: "https://api.wit.ai".to_owned(),
        }
    }

    #[test]
    fn silence_is_detected_and_speech_is_not() {
        assert!(is_silent(&fake_wav(&[0, 0, 0, 0])));
        assert!(!is_silent(&fake_wav(&[0, 1200, 0, 0])));
        assert!(is_silent(&fake_wav(&[])));
    }

    #[test]
    fn sentence_wavs_merge_into_one_playable_wav() {
        let merged = merge_wavs(&[fake_wav(&[100, 200]), fake_wav(&[300])]).expect("merge");
        let parsed = parse_wav(&merged).expect("parse");
        assert_eq!(parsed.data_len, 6);
        assert!(!is_silent(&merged));
    }

    #[tokio::test]
    async fn a_silent_answer_stops_the_read_instead_of_playing() {
        let fake = Arc::new(FakeWit::new());
        fake.be_silent();
        let speech = WitSpeech::new(fake, config());
        let error = speech.speak("hello there", None).await.expect_err("silent");
        assert_eq!(error.code(), "wit_silent_audio");
    }

    #[tokio::test]
    async fn a_refused_token_is_loud_and_names_its_fix() {
        let fake = Arc::new(FakeWit::new());
        let mut wrong = config();
        wrong.token = Some(Secret::new("not-the-token"));
        let speech = WitSpeech::new(fake, wrong);
        let error = speech
            .speak("hello there", None)
            .await
            .expect_err("refused");
        assert_eq!(error.code(), "wit_unauthorized");
        assert!(!error.to_string().contains("not-the-token"));
    }

    #[tokio::test]
    async fn a_missing_token_file_setting_is_not_configured() {
        let fake = Arc::new(FakeWit::new());
        let mut bare = config();
        bare.token = None;
        let speech = WitSpeech::new(fake, bare);
        let error = speech.speak("hello there", None).await.expect_err("bare");
        assert_eq!(error.code(), "wit_not_configured");
        assert!(error.to_string().contains("wit.token_file"));
    }

    #[tokio::test]
    async fn a_silent_answer_is_never_cached() {
        let fake = Arc::new(FakeWit::new());
        let speech = WitSpeech::new(fake.clone(), config());
        fake.be_silent();
        assert!(speech.speak("again after silence", None).await.is_err());
        fake.speak();
        speech
            .speak("again after silence", None)
            .await
            .expect("the vendor is asked again");
        assert_eq!(fake.requests().len(), 2);
    }

    #[test]
    fn a_wav_with_no_rate_is_not_playable_audio() {
        let mut wav = fake_wav(&[500; 4]);
        wav[24..28].copy_from_slice(&0_u32.to_le_bytes());
        assert!(merge_wavs(&[wav]).is_err());
    }

    #[tokio::test]
    async fn repeated_text_is_served_from_the_cache() {
        let fake = Arc::new(FakeWit::new());
        let speech = WitSpeech::new(fake.clone(), config());
        speech.speak("cache me once", None).await.expect("first");
        speech.speak("cache me once", None).await.expect("second");
        assert_eq!(fake.requests().len(), 1);
    }
}
