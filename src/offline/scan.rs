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
///   inspect `effective_providers()` on `asr` and `tts`, and add the
///   matching `OfflineModelKind` for any offline provider name found.
/// - A failed playbook load is propagated as an error — the deploy is
///   broken regardless and we'd rather fail at startup than silently
///   miss an offline reference.
///
/// LLM offline support (section 7, Candle) is not yet implemented;
/// when it lands, scan `llm.effective_providers()` here too.
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
        // TODO(section 7): scan llm.effective_providers() for offline-llm
        // once Candle lands and an OfflineModelKind variant is added.
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
