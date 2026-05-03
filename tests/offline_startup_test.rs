//! Integration tests for the startup-time offline-model reference scan
//! and eager init flow (resilient-voice-pipeline tasks 6.1–6.5).
//!
//! Verifies that when a playbook references an offline model whose files
//! are missing on disk, startup fails with a clear actionable error —
//! catching a misconfigured deploy at boot rather than mid-call during
//! cloud-to-offline failover.

#![cfg(feature = "offline")]

use active_call::config::{Config, InviteHandlerConfig};
use active_call::offline::{
    OfflineConfig, OfflineModelKind, OfflineModels, collect_referenced_offline_models,
};
use std::collections::HashSet;
use std::path::Path;

fn write_playbook(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write playbook");
    path
}

fn config_with_default_playbook(name: &str) -> Config {
    let mut c = Config::default();
    c.handler = Some(InviteHandlerConfig::Playbook {
        rules: None,
        default: Some(name.to_string()),
    });
    c
}

/// 6.5 (primary): a playbook references sensevoice but the models_dir
/// is empty, so eager init must error and surface the missing model.
#[tokio::test]
async fn startup_fails_when_referenced_offline_model_is_missing() {
    let pb_dir = tempfile::tempdir().expect("playbook tempdir");
    write_playbook(
        pb_dir.path(),
        "needs_sv.md",
        "---\nasr:\n  provider: sensevoice\n---\n# scene\nhi",
    );
    let config = config_with_default_playbook("needs_sv.md");

    // Empty models dir — sensevoice files are absent.
    let models_dir = tempfile::tempdir().expect("models tempdir");
    let offline_config = OfflineConfig::new(models_dir.path().to_path_buf(), 1);
    let models = OfflineModels::new(offline_config);

    let refs = collect_referenced_offline_models(&config, pb_dir.path())
        .await
        .expect("scan must succeed even if models are missing");
    assert!(
        refs.contains(&OfflineModelKind::Sensevoice),
        "scan must detect the sensevoice reference"
    );

    let err = models
        .eager_init_referenced(&refs)
        .await
        .expect_err("eager init must fail when the referenced model is missing");
    let msg = format!("{err}");
    assert!(
        msg.contains("sensevoice"),
        "error should name the model: {msg}"
    );
    assert!(
        msg.contains("--download-models"),
        "error should suggest the fix: {msg}"
    );
}

/// 6.5 (variant): cloud-only playbook → empty referenced set → eager
/// init is a no-op, even though models_dir is empty. Verifies we don't
/// accidentally fail startup for deploys that don't use offline at all.
#[tokio::test]
async fn startup_succeeds_when_no_offline_references() {
    let pb_dir = tempfile::tempdir().expect("playbook tempdir");
    write_playbook(
        pb_dir.path(),
        "cloud.md",
        "---\nasr:\n  provider: tencent\ntts:\n  provider: aliyun\n---\n# scene\nhi",
    );
    let config = config_with_default_playbook("cloud.md");

    let models_dir = tempfile::tempdir().expect("models tempdir");
    let offline_config = OfflineConfig::new(models_dir.path().to_path_buf(), 1);
    let models = OfflineModels::new(offline_config);

    let refs = collect_referenced_offline_models(&config, pb_dir.path())
        .await
        .expect("scan succeeds for cloud-only playbook");
    assert!(refs.is_empty(), "cloud-only playbook references nothing");

    models
        .eager_init_referenced(&refs)
        .await
        .expect("empty refs must be a no-op even with empty models_dir");
}

/// 6.5 (variant): webhook handler skips the playbook scan entirely.
#[tokio::test]
async fn webhook_handler_skips_offline_scan() {
    let mut config = Config::default();
    config.handler = Some(InviteHandlerConfig::Webhook {
        url: Some("https://example.invalid/h".into()),
        urls: None,
        method: None,
        headers: None,
    });

    let pb_dir = tempfile::tempdir().expect("playbook tempdir");
    let refs = collect_referenced_offline_models(&config, pb_dir.path())
        .await
        .expect("webhook scan succeeds");
    assert!(
        refs.is_empty(),
        "webhook handler does not reference any playbook"
    );
    assert_eq!(refs, HashSet::new());
}
