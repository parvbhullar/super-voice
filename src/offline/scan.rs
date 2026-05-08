//! Scan playbook provider chains for offline-model references.
//!
//! Lives in the lib (not main.rs) so integration tests can drive the
//! same scan logic the binary uses at startup. Kept feature-gated on
//! `offline` because the result type is offline-only.

use crate::config::{Config, InviteHandlerConfig};
use crate::offline::OfflineModelKind;
use crate::playbook::Playbook;
use crate::synthesis::SynthesisType;
use crate::transcription::TranscriptionType;
use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Scan all playbook provider chains and return the set of offline
/// models referenced by any of them.
///
/// `playbook_base_dir` is prepended to relative playbook paths from
/// config (matches `config/playbook` in production deploys; tests pass
/// a tempdir).
///
/// Behaviour:
/// - Webhook handler → empty set (no playbooks involved).
/// - Playbook handler → load each referenced .md (default + rules),
///   inspect `effective_providers()` on `asr`, `tts`, and `llm`, and
///   add the matching `OfflineModelKind` for any offline provider name
///   found.
/// - LLM provider name `phi3` (case-insensitive) maps to
///   `OfflineModelKind::Llm`. The scan returns the variant even when
///   the binary was built without `offline-llm`; eager_init_referenced
///   then surfaces a clear "feature not built in" error rather than
///   silently dropping the reference.
/// - A failed playbook load is propagated as an error — the deploy is
///   broken regardless and we'd rather fail at startup than silently
///   miss an offline reference.
pub async fn collect_referenced_offline_models(
    config: &Config,
    playbook_base_dir: &Path,
) -> Result<HashSet<OfflineModelKind>> {
    let mut refs = HashSet::new();
    let Some(InviteHandlerConfig::Playbook { rules, default }) = &config.handler else {
        return Ok(refs);
    };

    let mut names: Vec<String> = Vec::new();
    if let Some(d) = default {
        names.push(d.clone());
    }
    if let Some(rules) = rules {
        for rule in rules {
            names.push(rule.playbook.clone());
        }
    }

    for name in names {
        let path = resolve_playbook_path(&name, playbook_base_dir);
        let playbook = Playbook::load(&path).await.map_err(|e| {
            anyhow::anyhow!(
                "failed to load playbook {} during offline-reference scan: {}",
                path.display(),
                e
            )
        })?;

        if let Some(asr) = &playbook.config.asr {
            for provider in asr.effective_providers() {
                if matches!(provider, TranscriptionType::Sensevoice) {
                    refs.insert(OfflineModelKind::Sensevoice);
                }
            }
        }
        if let Some(tts) = &playbook.config.tts {
            for provider in tts.effective_providers() {
                if matches!(provider, SynthesisType::Supertonic) {
                    refs.insert(OfflineModelKind::Supertonic);
                }
            }
        }
        if let Some(llm) = &playbook.config.llm {
            for entry in llm.effective_providers() {
                if is_offline_llm_provider(&entry.provider) {
                    refs.insert(OfflineModelKind::Llm);
                }
                if is_offline_gemma4_provider(&entry.provider) {
                    refs.insert(OfflineModelKind::Gemma4);
                }
            }
        }
    }

    Ok(refs)
}

fn resolve_playbook_path(name: &str, base: &Path) -> PathBuf {
    // Backwards-compat: if config already includes the base prefix,
    // honour it as-is. Otherwise treat the name as relative to base.
    let p = Path::new(name);
    if p.is_absolute() || name.starts_with("config/playbook/") {
        PathBuf::from(name)
    } else {
        base.join(name)
    }
}

/// Returns true if the given LLM provider name designates the
/// in-process Candle-backed offline LLM. Recognised aliases:
/// `phi3`, `phi-3`, `candle`, `offline-llm` (all case-insensitive).
pub fn is_offline_llm_provider(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "phi3" | "phi-3" | "candle" | "offline-llm"
    )
}

/// Returns true if the given LLM provider name designates the
/// in-process llama.cpp-backed Gemma 4 model. Recognised aliases:
/// `gemma4`, `gemma-4`, `gemma` (all case-insensitive).
pub fn is_offline_gemma4_provider(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "gemma4" | "gemma-4" | "gemma"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlaybookRule;

    fn write_playbook(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn config_with_playbook_default(name: &str) -> Config {
        let mut c = Config::default();
        c.handler = Some(InviteHandlerConfig::Playbook {
            rules: None,
            default: Some(name.to_string()),
        });
        c
    }

    #[tokio::test]
    async fn webhook_handler_scans_to_empty_set() {
        let mut c = Config::default();
        c.handler = Some(InviteHandlerConfig::Webhook {
            url: Some("https://example.invalid/h".into()),
            urls: None,
            method: None,
            headers: None,
        });
        let tmp = tempfile::tempdir().unwrap();
        let refs = collect_referenced_offline_models(&c, tmp.path())
            .await
            .unwrap();
        assert!(refs.is_empty());
    }

    #[tokio::test]
    async fn no_handler_scans_to_empty_set() {
        let c = Config::default();
        let tmp = tempfile::tempdir().unwrap();
        let refs = collect_referenced_offline_models(&c, tmp.path())
            .await
            .unwrap();
        assert!(refs.is_empty());
    }

    #[tokio::test]
    async fn playbook_with_sensevoice_asr_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        // Minimal playbook with sensevoice as the ASR provider.
        write_playbook(
            tmp.path(),
            "sv.md",
            "---\nasr:\n  provider: sensevoice\n---\n# scene\nhello",
        );
        let c = config_with_playbook_default("sv.md");
        let refs = collect_referenced_offline_models(&c, tmp.path())
            .await
            .unwrap();
        assert!(refs.contains(&OfflineModelKind::Sensevoice));
        assert!(!refs.contains(&OfflineModelKind::Supertonic));
    }

    #[tokio::test]
    async fn playbook_with_supertonic_tts_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        write_playbook(
            tmp.path(),
            "st.md",
            "---\ntts:\n  provider: supertonic\n---\n# scene\nhello",
        );
        let c = config_with_playbook_default("st.md");
        let refs = collect_referenced_offline_models(&c, tmp.path())
            .await
            .unwrap();
        assert!(refs.contains(&OfflineModelKind::Supertonic));
        assert!(!refs.contains(&OfflineModelKind::Sensevoice));
    }

    #[tokio::test]
    async fn cloud_only_playbook_scans_to_empty_set() {
        let tmp = tempfile::tempdir().unwrap();
        write_playbook(
            tmp.path(),
            "cloud.md",
            "---\nasr:\n  provider: tencent\ntts:\n  provider: aliyun\n---\n# scene\nhi",
        );
        let c = config_with_playbook_default("cloud.md");
        let refs = collect_referenced_offline_models(&c, tmp.path())
            .await
            .unwrap();
        assert!(
            refs.is_empty(),
            "cloud-only playbook should not reference any offline models, got {refs:?}"
        );
    }

    #[tokio::test]
    async fn fallback_chain_with_offline_tail_is_detected() {
        // Multi-provider chain that ends in offline — exactly the
        // failover scenario eager init exists to support.
        let tmp = tempfile::tempdir().unwrap();
        write_playbook(
            tmp.path(),
            "chain.md",
            "---\nasr:\n  providers: [tencent, sensevoice]\ntts:\n  providers: [aliyun, supertonic]\n---\n# scene\nhi",
        );
        let c = config_with_playbook_default("chain.md");
        let refs = collect_referenced_offline_models(&c, tmp.path())
            .await
            .unwrap();
        assert!(refs.contains(&OfflineModelKind::Sensevoice));
        assert!(refs.contains(&OfflineModelKind::Supertonic));
    }

    #[tokio::test]
    async fn rules_and_default_are_both_scanned() {
        let tmp = tempfile::tempdir().unwrap();
        write_playbook(
            tmp.path(),
            "default.md",
            "---\nasr:\n  provider: tencent\n---\n# scene\nhi",
        );
        write_playbook(
            tmp.path(),
            "rule.md",
            "---\ntts:\n  provider: supertonic\n---\n# scene\nhi",
        );
        let mut c = Config::default();
        c.handler = Some(InviteHandlerConfig::Playbook {
            rules: Some(vec![PlaybookRule {
                caller: None,
                callee: None,
                playbook: "rule.md".into(),
            }]),
            default: Some("default.md".into()),
        });
        let refs = collect_referenced_offline_models(&c, tmp.path())
            .await
            .unwrap();
        assert!(
            refs.contains(&OfflineModelKind::Supertonic),
            "rule playbook reference must be picked up alongside default"
        );
    }

    #[test]
    fn is_offline_llm_provider_recognises_aliases() {
        assert!(is_offline_llm_provider("phi3"));
        assert!(is_offline_llm_provider("PHI-3"));
        assert!(is_offline_llm_provider("Candle"));
        assert!(is_offline_llm_provider("offline-llm"));
        assert!(!is_offline_llm_provider("openai"));
        assert!(!is_offline_llm_provider("phi"));
    }

    #[test]
    fn is_offline_gemma4_provider_recognises_aliases() {
        assert!(is_offline_gemma4_provider("gemma4"));
        assert!(is_offline_gemma4_provider("GEMMA-4"));
        assert!(is_offline_gemma4_provider("gemma"));
        assert!(!is_offline_gemma4_provider("openai"));
        assert!(!is_offline_gemma4_provider("phi3"));
    }

    #[tokio::test]
    async fn playbook_with_gemma4_llm_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        write_playbook(
            tmp.path(),
            "g4.md",
            "---\nllm:\n  provider: gemma4\n---\n# scene\nhi",
        );
        let c = config_with_playbook_default("g4.md");
        let refs = collect_referenced_offline_models(&c, tmp.path())
            .await
            .unwrap();
        assert!(refs.contains(&OfflineModelKind::Gemma4));
        assert!(!refs.contains(&OfflineModelKind::Llm));
    }

    #[tokio::test]
    async fn cloud_to_gemma4_chain_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        write_playbook(
            tmp.path(),
            "g4_chain.md",
            "---\nllm:\n  providers:\n    - provider: openai\n    - provider: gemma4\n---\n# scene\nhi",
        );
        let c = config_with_playbook_default("g4_chain.md");
        let refs = collect_referenced_offline_models(&c, tmp.path())
            .await
            .unwrap();
        assert!(refs.contains(&OfflineModelKind::Gemma4));
    }

    #[tokio::test]
    async fn playbook_with_phi3_llm_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        write_playbook(
            tmp.path(),
            "phi.md",
            "---\nllm:\n  provider: phi3\n---\n# scene\nhi",
        );
        let c = config_with_playbook_default("phi.md");
        let refs = collect_referenced_offline_models(&c, tmp.path())
            .await
            .unwrap();
        assert!(refs.contains(&OfflineModelKind::Llm));
    }

    #[tokio::test]
    async fn cloud_to_phi3_llm_chain_is_detected() {
        // Cloud LLM with offline phi3 as the final fallback — the
        // primary scenario the offline LLM tier exists to support.
        let tmp = tempfile::tempdir().unwrap();
        write_playbook(
            tmp.path(),
            "llm_chain.md",
            "---\nllm:\n  providers:\n    - provider: openai\n    - provider: phi3\n---\n# scene\nhi",
        );
        let c = config_with_playbook_default("llm_chain.md");
        let refs = collect_referenced_offline_models(&c, tmp.path())
            .await
            .unwrap();
        assert!(refs.contains(&OfflineModelKind::Llm));
    }

    #[tokio::test]
    async fn missing_playbook_file_is_a_hard_error() {
        let tmp = tempfile::tempdir().unwrap();
        // Reference a playbook that does not exist on disk.
        let c = config_with_playbook_default("nope.md");
        let err = collect_referenced_offline_models(&c, tmp.path())
            .await
            .expect_err("missing playbook file should be an error, not silent skip");
        let msg = format!("{err}");
        assert!(
            msg.contains("nope.md"),
            "error should name the missing playbook: {msg}"
        );
    }
}
