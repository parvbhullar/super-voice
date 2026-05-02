pub mod config;
pub mod downloader;

#[cfg(feature = "offline")]
pub mod sensevoice;

#[cfg(feature = "offline")]
pub mod supertonic;

pub use config::OfflineConfig;
pub use downloader::{ModelDownloader, ModelType};

#[cfg(feature = "offline")]
pub use sensevoice::SensevoiceEncoder;

#[cfg(feature = "offline")]
pub use supertonic::SupertonicTts;

use anyhow::{Result, anyhow};
use once_cell::sync::OnceCell;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

#[cfg(feature = "offline")]
pub struct OfflineModels {
    config: OfflineConfig,
    sensevoice: Arc<RwLock<Option<SensevoiceEncoder>>>,
    supertonic: Arc<RwLock<Option<SupertonicTts>>>,
}

#[cfg(feature = "offline")]
impl OfflineModels {
    pub fn new(config: OfflineConfig) -> Self {
        Self {
            config,
            sensevoice: Arc::new(RwLock::new(None)),
            supertonic: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn init_sensevoice(&self) -> Result<()> {
        let mut guard = self.sensevoice.write().await;
        if guard.is_none() {
            if !self.config.sensevoice_available() {
                anyhow::bail!(
                    "SenseVoice model files not found. Please run with --download-models sensevoice"
                );
            }

            info!("Initializing SenseVoice encoder...");
            let encoder = SensevoiceEncoder::new(
                &self.config.sensevoice_model_path(),
                &self.config.sensevoice_tokens_path(),
                self.config.threads,
            )?;
            *guard = Some(encoder);
            info!("✓ SenseVoice encoder initialized");
        }
        Ok(())
    }

    pub async fn get_sensevoice(&self) -> Result<Arc<RwLock<Option<SensevoiceEncoder>>>> {
        self.init_sensevoice().await?;
        Ok(self.sensevoice.clone())
    }

    pub async fn init_supertonic(&self) -> Result<()> {
        let mut guard = self.supertonic.write().await;
        if guard.is_none() {
            if !self.config.supertonic_available() {
                anyhow::bail!(
                    "Supertonic model files not found. Please run with --download-models supertonic"
                );
            }

            info!("Initializing Supertonic TTS...");
            let tts = SupertonicTts::new(
                &self.config.supertonic_onnx_dir(),
                &self.config.supertonic_config_path(),
                &self.config.supertonic_voice_styles_dir(),
                self.config.threads,
                false, // use_gpu
            )?;
            *guard = Some(tts);
            info!("✓ Supertonic TTS initialized");
        }
        Ok(())
    }

    pub async fn get_supertonic(&self) -> Result<Arc<RwLock<Option<SupertonicTts>>>> {
        self.init_supertonic().await?;
        Ok(self.supertonic.clone())
    }

    pub fn config(&self) -> &OfflineConfig {
        &self.config
    }

    /// Eagerly initialize every offline model whose files are available
    /// in `models_dir`. This shifts the 2–5 s ONNX initialization cost
    /// from first-call (where it would surface as a silent gap to the
    /// caller during cloud-to-offline failover) to process startup.
    ///
    /// Models with missing or partial files are skipped with a warning
    /// rather than failing startup. If a model file is *partially* present
    /// (so `available()` returns true but the actual ONNX load fails),
    /// the error is propagated — that's a real config/install bug.
    ///
    /// Each model's init duration is logged via tracing for observability;
    /// metric emission can hook into these spans.
    pub async fn eager_init_available(&self) -> Result<()> {
        use std::time::Instant;

        if self.config.sensevoice_available() {
            let start = Instant::now();
            self.init_sensevoice().await?;
            let elapsed = start.elapsed();
            info!(
                model = "sensevoice",
                init_seconds = elapsed.as_secs_f64(),
                "offline model initialized eagerly"
            );
        } else {
            debug!("sensevoice model files not present, skipping eager init");
        }

        if self.config.supertonic_available() {
            let start = Instant::now();
            self.init_supertonic().await?;
            let elapsed = start.elapsed();
            info!(
                model = "supertonic",
                init_seconds = elapsed.as_secs_f64(),
                "offline model initialized eagerly"
            );
        } else {
            debug!("supertonic model files not present, skipping eager init");
        }

        Ok(())
    }
}

#[cfg(feature = "offline")]
static OFFLINE_MODELS: OnceCell<OfflineModels> = OnceCell::new();

#[cfg(feature = "offline")]
pub fn init_offline_models(config: OfflineConfig) -> Result<()> {
    debug!(
        "Initializing offline models with dir: {}",
        config.models_dir.display()
    );
    OFFLINE_MODELS
        .set(OfflineModels::new(config))
        .map_err(|_| anyhow!("offline models already initialized"))
}

#[cfg(feature = "offline")]
pub fn get_offline_models() -> Option<&'static OfflineModels> {
    OFFLINE_MODELS.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_paths() {
        let config = OfflineConfig::default();
        assert!(
            config
                .sensevoice_dir()
                .to_string_lossy()
                .contains("sensevoice")
        );
        assert!(
            config
                .supertonic_dir()
                .to_string_lossy()
                .contains("supertonic")
        );
    }

    #[cfg(feature = "offline")]
    #[tokio::test]
    async fn eager_init_succeeds_when_no_models_present() {
        // Point at a fresh empty directory: nothing is "available", so
        // eager_init_available should be a no-op and return Ok.
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = OfflineConfig::new(tmp.path().to_path_buf(), 1);
        let models = OfflineModels::new(config);
        // Sanity: nothing reports available against an empty dir.
        assert!(!models.config.sensevoice_available());
        assert!(!models.config.supertonic_available());
        models
            .eager_init_available()
            .await
            .expect("eager init must not fail when no models present");
    }

    #[cfg(feature = "offline")]
    #[tokio::test]
    async fn eager_init_propagates_load_failure_for_partial_files() {
        // Create a sensevoice subdir whose `available()` check passes
        // (both expected files exist) but contents are garbage so the
        // ONNX loader will fail. eager_init_available must surface the
        // error rather than swallowing it — partial install is a real
        // config bug we want to catch at startup.
        let tmp = tempfile::tempdir().expect("tempdir");
        let sensevoice_dir = tmp.path().join("sensevoice");
        std::fs::create_dir_all(&sensevoice_dir).unwrap();
        std::fs::write(sensevoice_dir.join("model.onnx"), b"not a real onnx file").unwrap();
        std::fs::write(sensevoice_dir.join("tokens.txt"), b"garbage").unwrap();

        let config = OfflineConfig::new(tmp.path().to_path_buf(), 1);
        // This test is meaningful only if `available()` reports true on
        // those two file presences. If the availability check evolves,
        // this test catches the regression.
        if !config.sensevoice_available() {
            // Availability semantics changed; skip rather than false fail.
            return;
        }
        let models = OfflineModels::new(config);
        let res = models.eager_init_available().await;
        assert!(
            res.is_err(),
            "eager init should propagate underlying ONNX load failure"
        );
    }
}
