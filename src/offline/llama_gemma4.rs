//! In-process Gemma 4 GGUF LLM via llama-cpp-2 (llama.cpp Rust bindings).
//!
//! This is the offline Gemma 4 tier of the resilient pipeline. It serves as
//! an alternative to the Candle Phi-3 tier when a larger, more capable model
//! is preferred and the host has enough RAM (≥3 GB for the Q4_K_M 2B variant).
//!
//! Design notes:
//! - llama.cpp handles GGUF loading and hardware acceleration (Metal on macOS,
//!   CUDA when available, CPU otherwise). No separate runtime process needed.
//! - `LlamaGemma4State` holds the backend and model. A fresh `LlamaContext`
//!   (KV cache) is created per call, so concurrent offline requests can run
//!   in parallel — unlike Candle's single-writer pattern. In practice the
//!   fallback tier is rarely hot, so per-call context allocation is fine.
//! - Tool calls are dropped: Gemma 4 IT understands OpenAI function-calling
//!   JSON but the offline tier is a last-resort degraded path. Tool schema
//!   is silently ignored and a plain text response is returned.
//! - Per-call budget (`max_tokens`, `max_inference`) caps worst-case latency.
//! - Chat template: Gemma IT format (`<start_of_turn>` / `<end_of_turn>`).

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use futures::Stream;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel, Special};
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::offline::get_offline_models;
use crate::playbook::handler::provider::{LlmProvider, LlmStreamEvent};
use crate::playbook::{ChatMessage, LlmConfig};

/// Loaded Gemma 4 state. Owns the llama.cpp backend and model weights.
///
/// The backend must outlive the model; both are `Send + Sync` in llama-cpp-2,
/// so they can be shared across threads (each caller gets its own context).
pub struct LlamaGemma4State {
    pub backend: Arc<LlamaBackend>,
    pub model: Arc<LlamaModel>,
}

impl LlamaGemma4State {
    /// Load a GGUF model from disk. Blocking — call from `spawn_blocking`
    /// or at startup before the runtime is hot.
    pub fn load(gguf_path: &Path) -> Result<Self> {
        let backend = LlamaBackend::init()
            .context("failed to initialize llama.cpp backend")?;
        let backend = Arc::new(backend);

        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(&backend, gguf_path, &model_params)
            .with_context(|| format!("loading Gemma 4 GGUF from {}", gguf_path.display()))?;
        let model = Arc::new(model);

        Ok(Self { backend, model })
    }
}

/// Per-request budget. Caps worst-case time an offline call can take.
#[derive(Debug, Clone, Copy)]
pub struct Gemma4Budget {
    /// Maximum new tokens to generate (excl. prompt).
    pub max_tokens: usize,
    /// Hard wall-clock deadline for the whole generation.
    pub max_inference: Duration,
    /// Context window size in tokens (prompt + generation must fit).
    pub ctx_size: u32,
}

impl Default for Gemma4Budget {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            max_inference: Duration::from_secs(30),
            ctx_size: 2048,
        }
    }
}

/// `LlmProvider` impl backed by the in-process llama.cpp Gemma 4 model.
pub struct LlamaGemma4Provider {
    budget: Gemma4Budget,
}

impl Default for LlamaGemma4Provider {
    fn default() -> Self {
        Self::new()
    }
}

impl LlamaGemma4Provider {
    pub fn new() -> Self {
        Self {
            budget: Gemma4Budget::default(),
        }
    }

    pub fn with_budget(budget: Gemma4Budget) -> Self {
        Self { budget }
    }

    /// Render Gemma IT chat template. Tool schemas are dropped — the
    /// offline tier returns plain text only.
    fn render_prompt(history: &[ChatMessage]) -> String {
        let mut s = String::new();
        for msg in history {
            let role = match msg.role.as_str() {
                "assistant" => "model",
                _ => "user",
            };
            s.push_str("<start_of_turn>");
            s.push_str(role);
            s.push('\n');
            s.push_str(&msg.content);
            s.push_str("<end_of_turn>\n");
        }
        s.push_str("<start_of_turn>model\n");
        s
    }
}

#[async_trait]
impl LlmProvider for LlamaGemma4Provider {
    async fn call(&self, _config: &LlmConfig, history: &[ChatMessage]) -> Result<String> {
        let prompt = Self::render_prompt(history);
        let budget = self.budget;

        let result = tokio::task::spawn_blocking(move || -> Result<String> {
            let models = get_offline_models()
                .ok_or_else(|| anyhow!("OfflineModels not initialized"))?;
            let state_arc = futures::executor::block_on(models.get_gemma4())?;
            let state = state_arc.as_ref();
            run_inference_collect(state, &prompt, budget)
        })
        .await
        .map_err(|e| anyhow!("offline-gemma4 join error: {e}"))??;

        Ok(result)
    }

    async fn call_stream(
        &self,
        _config: &LlmConfig,
        history: &[ChatMessage],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent>> + Send>>> {
        let prompt = Self::render_prompt(history);
        let budget = self.budget;
        let (tx, rx) = mpsc::channel::<Result<String>>(64);

        tokio::task::spawn_blocking(move || {
            let result: Result<()> = (|| {
                let models = get_offline_models()
                    .ok_or_else(|| anyhow!("OfflineModels not initialized"))?;
                let state_arc = futures::executor::block_on(models.get_gemma4())?;
                let state = state_arc.as_ref();
                run_inference_stream(state, &prompt, budget, &tx)
            })();
            if let Err(e) = result {
                let _ = tx.blocking_send(Err(e));
            }
        });

        let stream = async_stream::stream! {
            let mut rx = rx;
            while let Some(item) = rx.recv().await {
                match item {
                    Ok(text) => yield Ok(LlmStreamEvent::Content(text)),
                    Err(e) => { yield Err(e); break; }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

fn run_inference_collect(
    state: &LlamaGemma4State,
    prompt: &str,
    budget: Gemma4Budget,
) -> Result<String> {
    let mut buf = String::new();
    inference_loop(state, prompt, budget, |piece| {
        buf.push_str(piece);
        Ok(())
    })?;
    Ok(buf)
}

fn run_inference_stream(
    state: &LlamaGemma4State,
    prompt: &str,
    budget: Gemma4Budget,
    tx: &mpsc::Sender<Result<String>>,
) -> Result<()> {
    inference_loop(state, prompt, budget, |piece| {
        match tx.blocking_send(Ok(piece.to_string())) {
            Ok(()) => Ok(()),
            Err(_) => Err(anyhow!("offline-gemma4 stream receiver dropped")),
        }
    })
}

/// Core generation loop. Creates a fresh `LlamaContext` (KV cache) for this
/// call, tokenizes the prompt, prefills, then decodes token-by-token.
fn inference_loop(
    state: &LlamaGemma4State,
    prompt: &str,
    budget: Gemma4Budget,
    mut on_piece: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let started = Instant::now();

    // Context per call — each gets its own KV cache.
    let ctx_size = NonZeroU32::new(budget.ctx_size).unwrap_or(NonZeroU32::new(2048).unwrap());
    let ctx_params = LlamaContextParams::default().with_n_ctx(Some(ctx_size));
    let mut ctx = state
        .model
        .new_context(&state.backend, ctx_params)
        .context("creating llama context for Gemma 4")?;

    // Tokenize prompt.
    let tokens = state
        .model
        .str_to_token(prompt, AddBos::Always)
        .context("tokenizing Gemma 4 prompt")?;

    if tokens.is_empty() {
        return Err(anyhow!("gemma4: empty token sequence after tokenization"));
    }

    let n_prompt = tokens.len();

    // Build initial batch with the full prompt.
    let mut batch = LlamaBatch::new(n_prompt + budget.max_tokens, 1);
    for (i, &token) in tokens.iter().enumerate() {
        let is_last = i == n_prompt - 1;
        batch
            .add(token, i as i32, &[0], is_last)
            .context("adding prompt token to batch")?;
    }
    ctx.decode(&mut batch).context("decoding prompt batch")?;

    // Greedy sampler (deterministic for fallback tier).
    let mut sampler = LlamaSampler::greedy();
    let mut n_cur = n_prompt;
    let mut generated = 0usize;

    loop {
        if generated >= budget.max_tokens {
            debug!(generated, "offline-gemma4 hit max_tokens budget; stopping");
            break;
        }
        if started.elapsed() >= budget.max_inference {
            warn!(
                generated,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "offline-gemma4 hit max_inference budget; stopping"
            );
            break;
        }

        let token = sampler.sample(&ctx, (batch.n_tokens() - 1) as i32);
        sampler.accept(token);

        if token == state.model.token_eos() {
            debug!("offline-gemma4 hit EOS after {generated} tokens");
            break;
        }

        // Decode token to text and forward to caller.
        #[allow(deprecated)]
        let piece = state
            .model
            .token_to_str(token, Special::Tokenize)
            .context("detokenizing gemma4 token")?;
        if !piece.is_empty() {
            on_piece(&piece)?;
        }

        // Next decode step: single-token batch.
        batch.clear();
        batch
            .add(token, n_cur as i32, &[0], true)
            .context("adding generated token to batch")?;
        ctx.decode(&mut batch).context("decoding generated token")?;
        n_cur += 1;
        generated += 1;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_prompt_gemma_template_user_turn() {
        let history = vec![ChatMessage {
            role: "user".into(),
            content: "hello".into(),
        }];
        let p = LlamaGemma4Provider::render_prompt(&history);
        assert!(p.contains("<start_of_turn>user\nhello<end_of_turn>"));
        assert!(p.ends_with("<start_of_turn>model\n"));
    }

    #[test]
    fn render_prompt_gemma_template_assistant_role() {
        let history = vec![
            ChatMessage {
                role: "user".into(),
                content: "hi".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "hello!".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "bye".into(),
            },
        ];
        let p = LlamaGemma4Provider::render_prompt(&history);
        assert!(p.contains("<start_of_turn>model\nhello!<end_of_turn>"));
        assert!(p.ends_with("<start_of_turn>model\n"));
    }

    #[test]
    fn render_prompt_system_role_falls_back_to_user() {
        let history = vec![ChatMessage {
            role: "system".into(),
            content: "be concise".into(),
        }];
        let p = LlamaGemma4Provider::render_prompt(&history);
        assert!(
            p.contains("<start_of_turn>user\nbe concise<end_of_turn>"),
            "system role should fall back to user turn: {p}"
        );
    }

    #[test]
    fn budget_default_sane() {
        let b = Gemma4Budget::default();
        assert!(b.max_tokens >= 64);
        assert!(b.max_inference >= Duration::from_secs(10));
        assert!(b.ctx_size >= 512);
    }

    /// End-to-end smoke test. Requires the Gemma 4 GGUF at
    /// `OFFLINE_MODELS_DIR/gemma4/`. Marked `#[ignore]` — CI does not
    /// ship the 1.5 GB model file.
    #[tokio::test]
    #[ignore]
    async fn end_to_end_gemma4_call_smoke() -> anyhow::Result<()> {
        use crate::offline::{OfflineConfig, OfflineModels, init_offline_models};
        use std::path::PathBuf;

        let models_dir = std::env::var("OFFLINE_MODELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./models"));
        let config = OfflineConfig::new(models_dir, num_cpus::get().min(4));
        if !config.gemma4_available() {
            eprintln!(
                "skipping: gemma4 model not present at {}",
                config.gemma4_dir().display()
            );
            return Ok(());
        }
        let _ = init_offline_models(config);

        let provider = LlamaGemma4Provider::with_budget(Gemma4Budget {
            max_tokens: 16,
            max_inference: Duration::from_secs(60),
            ctx_size: 2048,
        });
        let llm_config = LlmConfig::default();
        let history = vec![ChatMessage {
            role: "user".into(),
            content: "Say 'hello' and nothing else.".into(),
        }];
        let out = provider.call(&llm_config, &history).await?;
        assert!(!out.is_empty(), "gemma4 must produce some output");
        Ok(())
    }
}
