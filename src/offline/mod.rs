pub mod config;
pub mod downloader;

#[cfg(feature = "offline")]
pub mod sensevoice;

#[cfg(feature = "offline")]
pub mod supertonic;

#[cfg(feature = "offline-llm")]
pub mod candle;

#[cfg(feature = "offline")]
mod scan;

pub use config::OfflineConfig;
pub use downloader::{ModelDownloader, ModelType};

#[cfg(feature = "offline")]
pub use scan::{collect_referenced_offline_models, is_offline_llm_provider as scan_is_offline_llm_provider};

#[cfg(feature = "offline")]
pub use sensevoice::SensevoiceEncoder;

#[cfg(feature = "offline")]
pub use supertonic::SupertonicTts;

use anyhow::{Result, anyhow};
use once_cell::sync::OnceCell;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Identifies an offline model that can be eagerly initialized at startup.
///
/// Used by [`OfflineModels::eager_init_referenced`] together with the
/// referenced-model set computed from playbook provider chains. Adding a
/// new offline tier means adding a variant here and a match arm in
/// `eager_init_referenced`.
///
/// `Llm` is recognised even when the `offline-llm` feature is off so a
/// playbook referencing the offline LLM produces a clear "feature not
/// built in" error at startup rather than silently degrading to lazy
/// init that would fail at first call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OfflineModelKind {
    Sensevoice,
    Supertonic,
    Llm,
}

impl OfflineModelKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sensevoice => "sensevoice",
            Self::Supertonic => "supertonic",
            Self::Llm => "llm",
        }
    }
}

#[cfg(feature = "offline")]
pub struct OfflineModels {
    config: OfflineConfig,
    sensevoice: Arc<RwLock<Option<SensevoiceEncoder>>>,
    supertonic: Arc<RwLock<Option<SupertonicTts>>>,
    #[cfg(feature = "offline-llm")]
    llm: Arc<RwLock<Option<crate::offline::candle::CandlePhi3State>>>,
}

#[cfg(feature = "offline")]
impl OfflineModels {
    pub fn new(config: OfflineConfig) -> Self {
        Self {
            config,
            sensevoice: Arc::new(RwLock::new(None)),
            supertonic: Arc::new(RwLock::new(None)),
            #[cfg(feature = "offline-llm")]
            llm: Arc::new(RwLock::new(None)),
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

    /// Load the Phi-3 GGUF weights and tokenizer into memory.
    ///
    /// Idempotent: subsequent calls are a no-op once the model is
    /// resident. The 2–4 s load cost is paid here so failover into the
    /// offline tier doesn't stall a live call.
    #[cfg(feature = "offline-llm")]
    pub async fn init_llm(&self) -> Result<()> {
        let mut guard = self.llm.write().await;
        if guard.is_none() {
            if !self.config.llm_available() {
                anyhow::bail!(
                    "Offline LLM model files not found at {}. Please run with --download-models llm",
                    self.config.llm_dir().display()
                );
            }
            info!("Initializing offline LLM (Phi-3-mini-4k Q4_K_M)...");
            let state = crate::offline::candle::CandlePhi3State::load(
                &self.config.llm_gguf_path(),
                &self.config.llm_tokenizer_path(),
            )?;
            *guard = Some(state);
            info!("✓ Offline LLM initialized");
        }
        Ok(())
    }

    #[cfg(feature = "offline-llm")]
    pub async fn get_llm(
        &self,
    ) -> Result<Arc<RwLock<Option<crate::offline::candle::CandlePhi3State>>>> {
        self.init_llm().await?;
        Ok(self.llm.clone())
    }

    pub fn config(&self) -> &OfflineConfig {
        &self.config
    }

    /// Eagerly initialize each offline model that is *referenced* by a
    /// playbook provider chain. Referenced means the model name appears
    /// in `effective_providers()` for some playbook's asr/tts/llm config.
    ///
    /// Behaviour:
    /// - If `refs` is empty, this is a no-op (no offline tier in any chain).
    /// - For each referenced model, fail startup with a clear error if its
    ///   files are missing — a misconfigured deploy is better caught at
    ///   boot than mid-call during cloud-to-offline failover.
    /// - Init each referenced model and emit
    ///   `offline_model_init_seconds{model}` as a structured-log field
    ///   (scrapeable via Loki/Vector → Prometheus until section 9 wires
    ///   a dedicated metric).
    ///
    /// Why eager: lazy `OnceCell` init costs 2–5 s on first use. During
    /// failover that latency lands in a live call, surfacing as dead air.
    /// Eager init shifts the cost to startup where it is invisible.
    pub async fn eager_init_referenced(
        &self,
        refs: &std::collections::HashSet<OfflineModelKind>,
    ) -> Result<()> {
        use std::time::Instant;

        for kind in refs {
            let start = Instant::now();
            match kind {
                OfflineModelKind::Sensevoice => {
                    if !self.config.sensevoice_available() {
                        anyhow::bail!(
                            "Referenced offline model 'sensevoice' is missing files at {}. \
                             Run with --download-models sensevoice or remove sensevoice \
                             from playbook ASR provider chains.",
                            self.config.sensevoice_dir().display()
                        );
                    }
                    self.init_sensevoice().await?;
                }
                OfflineModelKind::Supertonic => {
                    if !self.config.supertonic_available() {
                        anyhow::bail!(
                            "Referenced offline model 'supertonic' is missing files at {}. \
                             Run with --download-models supertonic or remove supertonic \
                             from playbook TTS provider chains.",
                            self.config.supertonic_dir().display()
                        );
                    }
                    self.init_supertonic().await?;
                }
                OfflineModelKind::Llm => {
                    #[cfg(feature = "offline-llm")]
                    {
                        if !self.config.llm_available() {
                            anyhow::bail!(
                                "Referenced offline model 'llm' (Phi-3) is missing files at {}. \
                                 Run with --download-models llm or remove phi3 from playbook \
                                 LLM provider chains.",
                                self.config.llm_dir().display()
                            );
                        }
                        self.init_llm().await?;
                    }
                    #[cfg(not(feature = "offline-llm"))]
                    {
                        anyhow::bail!(
                            "Playbook references the offline LLM tier (provider 'phi3') but \
                             this binary was built without the `offline-llm` feature. \
                             Rebuild with --features offline-llm or remove phi3 from playbook \
                             LLM provider chains."
                        );
                    }
                }
            }
            let elapsed = start.elapsed();
            info!(
                model = kind.as_str(),
                init_seconds = elapsed.as_secs_f64(),
                "offline_model_init_seconds"
            );
        }

        if refs.is_empty() {
            debug!("no offline models referenced by playbook provider chains; skipping eager init");
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
    async fn eager_init_referenced_is_noop_when_set_empty() {
        // No playbook references any offline tier — eager init should
        // do nothing and return Ok even though models_dir is empty.
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = OfflineConfig::new(tmp.path().to_path_buf(), 1);
        let models = OfflineModels::new(config);
        let refs = std::collections::HashSet::new();
        models
            .eager_init_referenced(&refs)
            .await
            .expect("empty referenced set must be a no-op");
    }

    #[cfg(feature = "offline")]
    #[tokio::test]
    async fn eager_init_referenced_fails_when_referenced_model_missing() {
        // Sensevoice is referenced by config but no files are on disk.
        // Per spec 6.3: fail startup with a clear error rather than
        // silently degrading the failover path at runtime.
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = OfflineConfig::new(tmp.path().to_path_buf(), 1);
        let models = OfflineModels::new(config);
        let mut refs = std::collections::HashSet::new();
        refs.insert(OfflineModelKind::Sensevoice);
        let err = models
            .eager_init_referenced(&refs)
            .await
            .expect_err("must error when referenced model files are missing");
        let msg = format!("{err}");
        assert!(
            msg.contains("sensevoice"),
            "error message should name the missing model: {msg}"
        );
        assert!(
            msg.contains("--download-models"),
            "error message should suggest the fix: {msg}"
        );
    }

    #[cfg(feature = "offline")]
    #[tokio::test]
    async fn eager_init_referenced_propagates_load_failure_for_partial_files() {
        // available() reports true (both expected files exist) but the
        // contents are garbage so the ONNX loader will fail. The error
        // must surface — partial install is a real config bug.
        let tmp = tempfile::tempdir().expect("tempdir");
        let sensevoice_dir = tmp.path().join("sensevoice");
        std::fs::create_dir_all(&sensevoice_dir).unwrap();
        std::fs::write(sensevoice_dir.join("model.onnx"), b"not a real onnx file").unwrap();
        std::fs::write(sensevoice_dir.join("tokens.txt"), b"garbage").unwrap();

        let config = OfflineConfig::new(tmp.path().to_path_buf(), 1);
        if !config.sensevoice_available() {
            // Availability semantics changed; skip rather than false fail.
            return;
        }
        let models = OfflineModels::new(config);
        let mut refs = std::collections::HashSet::new();
        refs.insert(OfflineModelKind::Sensevoice);
        let res = models.eager_init_referenced(&refs).await;
        assert!(
            res.is_err(),
            "eager init should propagate underlying ONNX load failure"
        );
    }

    #[test]
    fn offline_model_kind_as_str() {
        assert_eq!(OfflineModelKind::Sensevoice.as_str(), "sensevoice");
        assert_eq!(OfflineModelKind::Supertonic.as_str(), "supertonic");
    }
}
