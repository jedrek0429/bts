//! Runtime speech rendering and content-addressed voice caching.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, bail};
use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::{fs, process::Command, sync::Mutex};
use uuid::Uuid;

const AUDIO_FORMAT: &str = "pcm-s16le-8000-mono-v1";

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceSettings {
    pub language: String,
    pub voice: String,
    pub model: String,
    pub model_version: String,
    pub speed: f32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            language: "en".into(),
            voice: "bf_emma".into(),
            model: "kokoro".into(),
            model_version: "unknown".into(),
            speed: 1.05,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderedPrompt {
    pub cache_key: String,
    pub cache_path: PathBuf,
    /// Asterisk media URI, without a filename extension.
    pub media_uri: String,
}

#[async_trait]
pub trait SpeechSynthesizer: Send + Sync + 'static {
    async fn synthesise(&self, text: &str, settings: &VoiceSettings) -> anyhow::Result<Vec<u8>>;
}

#[derive(Clone)]
pub struct KokoroSynthesizer {
    client: reqwest::Client,
    endpoint: String,
    ffmpeg: PathBuf,
}

impl KokoroSynthesizer {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: endpoint.into(),
            ffmpeg: PathBuf::from("ffmpeg"),
        }
    }
}

#[derive(Serialize)]
struct KokoroRequest<'a> {
    model: &'a str,
    voice: &'a str,
    input: &'a str,
    response_format: &'static str,
    speed: f32,
}

#[async_trait]
impl SpeechSynthesizer for KokoroSynthesizer {
    async fn synthesise(&self, text: &str, settings: &VoiceSettings) -> anyhow::Result<Vec<u8>> {
        let source = self
            .client
            .post(&self.endpoint)
            .json(&KokoroRequest {
                model: &settings.model,
                voice: &settings.voice,
                input: text,
                response_format: "wav",
                speed: settings.speed,
            })
            .send()
            .await
            .context("failed to contact the configured TTS endpoint")?
            .error_for_status()
            .context("the configured TTS endpoint rejected the prompt")?
            .bytes()
            .await
            .context("failed to read TTS audio")?;

        let temporary = tempfile::tempdir().context("failed to create TTS workspace")?;
        let source_path = temporary.path().join("source.wav");
        let output_path = temporary.path().join("telephone.wav");
        fs::write(&source_path, source)
            .await
            .context("failed to stage TTS audio")?;
        let output = Command::new(&self.ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(&source_path)
            .args(["-ar", "8000", "-ac", "1", "-c:a", "pcm_s16le"])
            .arg(&output_path)
            .output()
            .await
            .context("failed to run ffmpeg for TTS audio")?;
        if !output.status.success() {
            bail!(
                "ffmpeg could not convert TTS audio: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        fs::read(output_path)
            .await
            .context("failed to read converted TTS audio")
    }
}

pub struct VoiceCache<S> {
    synthesizer: S,
    settings: VoiceSettings,
    cache_dir: PathBuf,
    sounds_dir: PathBuf,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl<S: SpeechSynthesizer> VoiceCache<S> {
    pub fn new(
        synthesizer: S,
        settings: VoiceSettings,
        cache_dir: impl Into<PathBuf>,
        sounds_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            synthesizer,
            settings,
            cache_dir: cache_dir.into(),
            sounds_dir: sounds_dir.into(),
            locks: Mutex::new(HashMap::new()),
        }
    }

    pub async fn render(&self, text: &str) -> anyhow::Result<RenderedPrompt> {
        let key = cache_key(text, &self.settings);
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        let cache_path = self.cache_dir.join(format!("{key}.wav"));
        let sound_path = self.sounds_dir.join(format!("{key}.wav"));

        fs::create_dir_all(&self.cache_dir)
            .await
            .with_context(|| format!("failed to create {}", self.cache_dir.display()))?;
        fs::create_dir_all(&self.sounds_dir)
            .await
            .with_context(|| format!("failed to create {}", self.sounds_dir.display()))?;

        if !valid_wave(&cache_path).await {
            let audio = self
                .synthesizer
                .synthesise(text, &self.settings)
                .await
                .with_context(|| format!("failed to render voice prompt {text:?}"))?;
            if !valid_wave_bytes(&audio) {
                bail!("TTS returned malformed WAV audio for prompt {text:?}");
            }
            let temporary = self
                .cache_dir
                .join(format!(".{key}.{}.tmp", Uuid::new_v4()));
            fs::write(&temporary, audio)
                .await
                .with_context(|| format!("failed to write voice cache entry for {text:?}"))?;
            fs::rename(&temporary, &cache_path)
                .await
                .with_context(|| format!("failed to publish voice cache entry for {text:?}"))?;
        }

        if !valid_wave(&sound_path).await && fs::hard_link(&cache_path, &sound_path).await.is_err()
        {
            fs::copy(&cache_path, &sound_path)
                .await
                .with_context(|| format!("failed to expose cached voice prompt {text:?}"))?;
        }

        Ok(RenderedPrompt {
            cache_key: key.clone(),
            cache_path,
            media_uri: format!("sound:bts-generated/{key}"),
        })
    }
}

pub fn cache_key(text: &str, settings: &VoiceSettings) -> String {
    let mut hash = Sha256::new();
    for part in [
        text,
        &settings.language,
        &settings.voice,
        &settings.model,
        &settings.model_version,
        &settings.speed.to_string(),
        AUDIO_FORMAT,
    ] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    hex::encode(hash.finalize())
}

async fn valid_wave(path: &Path) -> bool {
    match fs::read(path).await {
        Ok(bytes) => valid_wave_bytes(&bytes),
        Err(_) => false,
    }
}

fn valid_wave_bytes(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Clone)]
    struct FakeSynthesizer(Arc<AtomicUsize>);

    #[async_trait]
    impl SpeechSynthesizer for FakeSynthesizer {
        async fn synthesise(
            &self,
            _text: &str,
            _settings: &VoiceSettings,
        ) -> anyhow::Result<Vec<u8>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(b"RIFF\0\0\0\0WAVEtest".to_vec())
        }
    }

    fn fixture(
        settings: VoiceSettings,
    ) -> (
        tempfile::TempDir,
        Arc<AtomicUsize>,
        VoiceCache<FakeSynthesizer>,
    ) {
        let root = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let cache = VoiceCache::new(
            FakeSynthesizer(calls.clone()),
            settings,
            root.path().join("cache"),
            root.path().join("sounds"),
        );
        (root, calls, cache)
    }

    #[tokio::test]
    async fn cache_misses_then_hits_and_varies_by_text() {
        let (_root, calls, cache) = fixture(VoiceSettings::default());
        let first = cache.render("Welcome.").await.unwrap();
        let repeated = cache.render("Welcome.").await.unwrap();
        let changed = cache.render("Welcome back.").await.unwrap();
        assert_eq!(first.cache_key, repeated.cache_key);
        assert_ne!(first.cache_key, changed.cache_key);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cache_key_includes_voice_and_all_rendering_inputs() {
        let first = VoiceSettings::default();
        let mut changed = first.clone();
        changed.voice = "af_heart".into();
        assert_ne!(cache_key("Hello", &first), cache_key("Hello", &changed));
    }

    #[tokio::test]
    async fn malformed_entries_are_regenerated() {
        let (_root, calls, cache) = fixture(VoiceSettings::default());
        let key = cache_key("Hello", &VoiceSettings::default());
        fs::create_dir_all(&cache.cache_dir).await.unwrap();
        fs::write(cache.cache_dir.join(format!("{key}.wav")), b"broken")
            .await
            .unwrap();
        cache.render("Hello").await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_requests_generate_one_cache_entry() {
        let (_root, calls, cache) = fixture(VoiceSettings::default());
        let (one, two, three) = tokio::join!(
            cache.render("Same prompt"),
            cache.render("Same prompt"),
            cache.render("Same prompt")
        );
        assert!(one.is_ok() && two.is_ok() && three.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    struct FailedSynthesizer;

    #[async_trait]
    impl SpeechSynthesizer for FailedSynthesizer {
        async fn synthesise(
            &self,
            _text: &str,
            _settings: &VoiceSettings,
        ) -> anyhow::Result<Vec<u8>> {
            bail!("TTS offline")
        }
    }

    #[tokio::test]
    async fn failed_tts_is_a_prompt_error_not_a_process_failure() {
        let root = tempfile::tempdir().unwrap();
        let cache = VoiceCache::new(
            FailedSynthesizer,
            VoiceSettings::default(),
            root.path().join("cache"),
            root.path().join("sounds"),
        );
        let error = cache.render("Optional description").await.unwrap_err();
        assert!(error.to_string().contains("Optional description"));
    }
}
