//! In-process Phi-3 GGUF LLM via Candle.
//!
//! This is the offline LLM tier of the resilient pipeline. It exists
//! so a deploy can serve traffic with no cloud LLM reachable, at
//! degraded quality, instead of dropping calls.
//!
//! Design notes:
//! - The model is held inside `OfflineModels` as a singleton; init is
//!   eager (section 6) and a cold call should never be the one that
//!   pays the 2–4 s GGUF load.
//! - `ModelWeights` is mutable through inference (KV cache lives in
//!   `layers[*].kv_cache`), so each call takes a write lock. This
//!   serialises offline LLM inference per process. That's acceptable
//!   for a fallback tier — horizontal scale comes from running more
//!   replicas, not multi-thread inference.
//! - Tool calls are dropped per design.md decision (Phi-3-mini handles
//!   them poorly): the wrapper ignores any tool schema in the request
//!   and returns a plain text response.
//! - Per-call budget (`max_tokens`, `max_inference_seconds`) bounds the
//!   worst case so a runaway generation can't hold the model lock.

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::quantized_phi3::ModelWeights;
use futures::Stream;
use std::path::Path;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tokenizers::Tokenizer;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::offline::get_offline_models;
use crate::playbook::handler::provider::{LlmProvider, LlmStreamEvent};
use crate::playbook::{ChatMessage, LlmConfig};

/// Loaded Phi-3 state. Owns weights, tokenizer, and target device.
///
/// Held inside a Tokio `RwLock<Option<...>>` on `OfflineModels`. Each
/// call takes a write lock and does inference; concurrent offline LLM
/// requests serialise at this lock by design.
pub struct CandlePhi3State {
    pub model: ModelWeights,
    pub tokenizer: Tokenizer,
    pub device: Device,
    /// Token id that signals end-of-turn for Phi-3 (`<|end|>`). Used to
    /// stop generation early when the model emits it.
    pub eos_token_id: u32,
}

impl CandlePhi3State {
    /// Load weights from a GGUF file and the tokenizer JSON.
    ///
    /// Blocking — call from within `tokio::task::spawn_blocking` or
    /// during startup before the runtime is hot. The 2–4 s cost is
    /// the whole reason eager init exists.
    pub fn load(gguf_path: &Path, tokenizer_path: &Path) -> Result<Self> {
        let device = Device::Cpu;
        let mut file = std::fs::File::open(gguf_path)
            .with_context(|| format!("opening GGUF at {}", gguf_path.display()))?;
        let content = gguf_file::Content::read(&mut file)
            .with_context(|| format!("reading GGUF metadata at {}", gguf_path.display()))?;
        let model = ModelWeights::from_gguf(false, content, &mut file, &device)
            .with_context(|| format!("loading Phi-3 weights from {}", gguf_path.display()))?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow!("loading tokenizer at {}: {}", tokenizer_path.display(), e))?;
        let eos_token_id = tokenizer
            .token_to_id("<|end|>")
            .or_else(|| tokenizer.token_to_id("<|endoftext|>"))
            .unwrap_or(2);
        Ok(Self {
            model,
            tokenizer,
            device,
            eos_token_id,
        })
    }
}

/// Per-request budget. Bounds the worst-case time the offline LLM can
/// hold its mutex, so a stuck generation doesn't park other callers.
#[derive(Debug, Clone, Copy)]
pub struct CandleBudget {
    pub max_tokens: usize,
    pub max_inference: Duration,
}

impl Default for CandleBudget {
    fn default() -> Self {
        Self {
            max_tokens: 256,
            max_inference: Duration::from_secs(15),
        }
    }
}

/// `LlmProvider` impl that routes through the in-process Candle Phi-3
/// model registered on `OfflineModels`. The provider itself is cheap
/// to construct — all state lives in the singleton.
pub struct CandlePhi3Provider {
    budget: CandleBudget,
}

impl Default for CandlePhi3Provider {
    fn default() -> Self {
        Self::new()
    }
}

impl CandlePhi3Provider {
    pub fn new() -> Self {
        Self {
            budget: CandleBudget::default(),
        }
    }

    pub fn with_budget(budget: CandleBudget) -> Self {
        Self { budget }
    }

    /// Render Phi-3's chat template. Ignores any `tools` field on the
    /// config — tool calls are dropped for the offline tier.
    fn render_prompt(history: &[ChatMessage]) -> String {
        let mut s = String::new();
        for msg in history {
            let role_tag = match msg.role.as_str() {
                "system" => "<|system|>",
                "assistant" => "<|assistant|>",
                _ => "<|user|>",
            };
            s.push_str(role_tag);
            s.push('\n');
            s.push_str(&msg.content);
            s.push_str("<|end|>\n");
        }
        s.push_str("<|assistant|>\n");
        s
    }
}

#[async_trait]
impl LlmProvider for CandlePhi3Provider {
    /// Non-streaming call. Renders the prompt, generates up to budget,
    /// returns the decoded text. Blocking inference runs on Tokio's
    /// blocking pool so the async runtime stays responsive.
    async fn call(&self, _config: &LlmConfig, history: &[ChatMessage]) -> Result<String> {
        let prompt = Self::render_prompt(history);
        let budget = self.budget;

        let result = tokio::task::spawn_blocking(move || -> Result<String> {
            let models = get_offline_models()
                .ok_or_else(|| anyhow!("OfflineModels not initialized"))?;
            // get_llm() goes through init_llm() which holds an async
            // lock; bridge through current-thread runtime.
            let state_lock = futures::executor::block_on(models.get_llm())?;
            let mut guard = futures::executor::block_on(state_lock.write());
            let state = guard
                .as_mut()
                .ok_or_else(|| anyhow!("offline LLM state missing after init"))?;
            run_inference_collect(state, &prompt, budget)
        })
        .await
        .map_err(|e| anyhow!("offline-llm join error: {e}"))??;

        Ok(result)
    }

    /// Streaming call. Spawns a blocking inference task that pushes
    /// tokens through an mpsc channel; the returned stream yields
    /// `LlmStreamEvent::Content` for each new token.
    async fn call_stream(
        &self,
        _config: &LlmConfig,
        history: &[ChatMessage],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent>> + Send>>> {
        let prompt = Self::render_prompt(history);
        let budget = self.budget;
        let (tx, rx) = mpsc::channel::<Result<String>>(32);

        tokio::task::spawn_blocking(move || {
            let result: Result<()> = (|| {
                let models = get_offline_models()
                    .ok_or_else(|| anyhow!("OfflineModels not initialized"))?;
                let state_lock = futures::executor::block_on(models.get_llm())?;
                let mut guard = futures::executor::block_on(state_lock.write());
                let state = guard
                    .as_mut()
                    .ok_or_else(|| anyhow!("offline LLM state missing after init"))?;
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
                    Err(e) => {
                        yield Err(e);
                        break;
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

/// Run inference and return the full decoded text. Used by the
/// non-streaming call path.
fn run_inference_collect(
    state: &mut CandlePhi3State,
    prompt: &str,
    budget: CandleBudget,
) -> Result<String> {
    let mut buf = String::new();
    inference_loop(state, prompt, budget, |piece| {
        buf.push_str(piece);
        Ok(())
    })?;
    Ok(buf)
}

/// Run inference and stream each decoded token piece through the
/// channel. Returns when generation completes (EOS, max_tokens, or
/// budget exhaustion) or when the receiver drops.
fn run_inference_stream(
    state: &mut CandlePhi3State,
    prompt: &str,
    budget: CandleBudget,
    tx: &mpsc::Sender<Result<String>>,
) -> Result<()> {
    inference_loop(state, prompt, budget, |piece| {
        // Best-effort send. If the receiver has dropped (caller went
        // away mid-stream), break the loop.
        match tx.blocking_send(Ok(piece.to_string())) {
            Ok(()) => Ok(()),
            Err(_) => Err(anyhow!("offline-llm stream receiver dropped")),
        }
    })
}

/// Core token-by-token decode loop, parameterised over what to do with
/// each newly-decoded piece. Bounded by `budget` and stops on EOS.
fn inference_loop(
    state: &mut CandlePhi3State,
    prompt: &str,
    budget: CandleBudget,
    mut on_piece: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let started = Instant::now();
    let encoded = state
        .tokenizer
        .encode(prompt, true)
        .map_err(|e| anyhow!("phi3 tokenize: {e}"))?;
    let tokens = encoded.get_ids().to_vec();
    if tokens.is_empty() {
        return Err(anyhow!("phi3: empty prompt after tokenization"));
    }

    let mut all_tokens: Vec<u32> = tokens.clone();
    let mut logits_processor = LogitsProcessor::new(0, Some(0.7), Some(0.95));

    // Prefill: feed the prompt in one forward pass to populate the KV
    // cache, take the logits over the last position to predict the
    // first new token.
    let input = Tensor::new(tokens.as_slice(), &state.device)?.unsqueeze(0)?;
    let logits = state.model.forward(&input, 0)?;
    let logits = squeeze_last(&logits)?;
    let mut next_token = logits_processor.sample(&logits)?;
    all_tokens.push(next_token);
    decode_and_emit(state, all_tokens.last().copied(), &mut on_piece)?;

    // Decode loop. KV cache makes each step a single-token forward.
    let mut generated = 1usize;
    let mut index_pos = tokens.len();
    while generated < budget.max_tokens {
        if started.elapsed() >= budget.max_inference {
            warn!(
                generated,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "offline-llm hit max_inference budget; stopping"
            );
            break;
        }
        if next_token == state.eos_token_id {
            debug!("offline-llm hit EOS after {generated} tokens");
            break;
        }
        let input = Tensor::new(&[next_token], &state.device)?.unsqueeze(0)?;
        let logits = state.model.forward(&input, index_pos)?;
        let logits = squeeze_last(&logits)?;
        next_token = logits_processor.sample(&logits)?;
        all_tokens.push(next_token);
        index_pos += 1;
        generated += 1;
        decode_and_emit(state, all_tokens.last().copied(), &mut on_piece)?;
    }
    Ok(())
}

fn squeeze_last(logits: &Tensor) -> Result<Tensor> {
    // forward() returns either (1, seq, vocab) for prefill or
    // (1, vocab) for single-token decode depending on the model
    // variant; normalise to (vocab,).
    let dims = logits.dims().to_vec();
    let logits = match dims.len() {
        3 => logits.i((.., dims[1] - 1, ..))?.squeeze(0)?,
        2 => logits.squeeze(0)?,
        _ => logits.clone(),
    };
    Ok(logits)
}

/// Decode the latest token id to text and forward it to the caller.
/// Uses `Tokenizer::decode` rather than `id_to_token` so byte-level
/// merges (common in Phi-3) are reassembled correctly.
fn decode_and_emit(
    state: &CandlePhi3State,
    next: Option<u32>,
    on_piece: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let Some(id) = next else {
        return Ok(());
    };
    if id == state.eos_token_id {
        return Ok(());
    }
    let piece = state
        .tokenizer
        .decode(&[id], false)
        .map_err(|e| anyhow!("phi3 detokenize: {e}"))?;
    if piece.is_empty() {
        return Ok(());
    }
    on_piece(&piece)
}

// Re-export Candle's `IndexOp` shim used by `squeeze_last`.
use candle_core::IndexOp;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playbook::ChatMessage;

    #[test]
    fn render_prompt_uses_phi3_template() {
        let history = vec![
            ChatMessage {
                role: "system".into(),
                content: "be brief".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "hi".into(),
            },
        ];
        let prompt = CandlePhi3Provider::render_prompt(&history);
        assert!(prompt.contains("<|system|>\nbe brief<|end|>"));
        assert!(prompt.contains("<|user|>\nhi<|end|>"));
        assert!(prompt.ends_with("<|assistant|>\n"));
    }

    #[test]
    fn render_prompt_normalises_unknown_role_to_user() {
        let history = vec![ChatMessage {
            role: "tool".into(),
            content: "result".into(),
        }];
        let prompt = CandlePhi3Provider::render_prompt(&history);
        assert!(
            prompt.contains("<|user|>\nresult<|end|>"),
            "unknown role should fall back to user: {prompt}"
        );
    }

    #[test]
    fn budget_default_has_sane_bounds() {
        let b = CandleBudget::default();
        assert!(b.max_tokens >= 64 && b.max_tokens <= 1024);
        assert!(b.max_inference >= Duration::from_secs(5));
        assert!(b.max_inference <= Duration::from_secs(60));
    }

    #[test]
    fn budget_with_explicit_values() {
        let b = CandleBudget {
            max_tokens: 32,
            max_inference: Duration::from_secs(2),
        };
        let p = CandlePhi3Provider::with_budget(b);
        assert_eq!(p.budget.max_tokens, 32);
        assert_eq!(p.budget.max_inference, Duration::from_secs(2));
    }

    /// Smoke test for the full inference path. Requires a real Phi-3
    /// GGUF and tokenizer at `OFFLINE_MODELS_DIR/llm/`. Marked
    /// `#[ignore]` because CI does not ship the 2.4 GB model.
    #[tokio::test]
    #[ignore]
    async fn end_to_end_phi3_call_smoke() -> anyhow::Result<()> {
        use crate::offline::{OfflineConfig, OfflineModels, init_offline_models};
        use std::path::PathBuf;

        let models_dir = std::env::var("OFFLINE_MODELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./models"));
        let config = OfflineConfig::new(models_dir, num_cpus::get().min(4));
        if !config.llm_available() {
            eprintln!(
                "skipping: phi3 model not present at {}",
                config.llm_dir().display()
            );
            return Ok(());
        }
        let _ = init_offline_models(config);

        let provider = CandlePhi3Provider::with_budget(CandleBudget {
            max_tokens: 16,
            max_inference: Duration::from_secs(30),
        });
        let llm_config = LlmConfig::default();
        let history = vec![ChatMessage {
            role: "user".into(),
            content: "Say 'hello' and nothing else.".into(),
        }];
        let out = provider.call(&llm_config, &history).await?;
        assert!(!out.is_empty(), "phi3 must produce some output");
        Ok(())
    }
}
