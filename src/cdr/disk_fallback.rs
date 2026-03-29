//! Disk fallback writer for CDRs that could not be delivered via webhook.
//!
//! CDRs are written as JSON files under hourly-rotated subdirectories:
//! `{fallback_dir}/{YYYYMMDD_HH}/{uuid}.json`.

use anyhow::Result;
use tokio::fs;
use tracing::info;

use crate::cdr::types::CarrierCdr;

/// Write a CDR to disk when all webhook deliveries have failed.
///
/// Creates the directory `{fallback_dir}/{YYYYMMDD_HH}/` (hourly rotation)
/// and writes `{uuid}.json` containing the serialized CDR.
///
/// Returns the full path of the written file.
pub async fn write_cdr_to_disk(fallback_dir: &str, cdr: &CarrierCdr) -> Result<String> {
    // Build hourly-rotated subdirectory: YYYYMMDD_HH
    let hour_dir = cdr.created_at.format("%Y%m%d_%H").to_string();
    let dir_path = format!("{}/{}", fallback_dir, hour_dir);

    fs::create_dir_all(&dir_path).await?;

    let file_path = format!("{}/{}.json", dir_path, cdr.uuid);
    let content = serde_json::to_string_pretty(cdr)?;

    fs::write(&file_path, content.as_bytes()).await?;

    info!(path = %file_path, uuid = %cdr.uuid, "CDR written to disk fallback");
    Ok(file_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    use crate::cdr::types::{CarrierCdr, CdrLeg, CdrStatus, CdrTiming};

    fn make_test_cdr() -> CarrierCdr {
        let now = Utc::now();
        CarrierCdr {
            uuid: Uuid::new_v4(),
            session_id: "sess-001".to_string(),
            call_id: "call-001".to_string(),
            node_id: "node-01".to_string(),
            created_at: now,
            inbound_leg: CdrLeg {
                trunk: "did-trunk".to_string(),
                gateway: None,
                caller: "+15551234567".to_string(),
                callee: "+15559876543".to_string(),
                codec: Some("PCMU".to_string()),
                transport: "udp".to_string(),
                srtp: false,
                sip_status: 200,
                hangup_cause: Some("NORMAL_CLEARING".to_string()),
                source_ip: Some("10.0.0.1".to_string()),
                destination_ip: Some("10.0.0.2".to_string()),
            },
            outbound_leg: None,
            timing: CdrTiming {
                start_time: now - chrono::Duration::seconds(60),
                ring_time: None,
                answer_time: None,
                end_time: now,
            },
            status: CdrStatus::Failed,
        }
    }

    /// Test 4: write_cdr_to_disk creates a JSON file at
    /// {fallback_dir}/{YYYYMMDD_HH}/{uuid}.json with CDR content.
    #[tokio::test]
    async fn test_write_cdr_to_disk_creates_json_file() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let cdr = make_test_cdr();
        let fallback_dir = tmp.path().to_str().unwrap();

        let path = write_cdr_to_disk(fallback_dir, &cdr)
            .await
            .expect("write_cdr_to_disk");

        // File must exist
        assert!(
            std::path::Path::new(&path).exists(),
            "CDR file should exist at {path}"
        );

        // File must contain valid CDR JSON
        let contents = tokio::fs::read_to_string(&path).await.expect("read file");
        let restored: CarrierCdr =
            serde_json::from_str(&contents).expect("deserialize CDR from disk");
        assert_eq!(restored.uuid, cdr.uuid);
        assert_eq!(restored.session_id, cdr.session_id);
    }

    /// Test 5: write_cdr_to_disk creates hourly-rotated subdirectories.
    #[tokio::test]
    async fn test_write_cdr_to_disk_creates_hourly_dir() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let cdr = make_test_cdr();
        let fallback_dir = tmp.path().to_str().unwrap();

        let path = write_cdr_to_disk(fallback_dir, &cdr)
            .await
            .expect("write_cdr_to_disk");

        // Path must include hourly directory YYYYMMDD_HH
        let expected_hour_dir = cdr.created_at.format("%Y%m%d_%H").to_string();
        assert!(
            path.contains(&expected_hour_dir),
            "path '{path}' should contain hourly dir '{expected_hour_dir}'"
        );

        // Path must end with {uuid}.json
        let expected_filename = format!("{}.json", cdr.uuid);
        assert!(
            path.ends_with(&expected_filename),
            "path '{path}' should end with '{expected_filename}'"
        );
    }

    /// Test: writing multiple CDRs in the same hour lands them in the same dir.
    #[tokio::test]
    async fn test_write_multiple_cdrs_same_hour_same_dir() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let fallback_dir = tmp.path().to_str().unwrap();

        let cdr1 = make_test_cdr();
        let cdr2 = make_test_cdr();

        let path1 = write_cdr_to_disk(fallback_dir, &cdr1)
            .await
            .expect("write cdr1");
        let path2 = write_cdr_to_disk(fallback_dir, &cdr2)
            .await
            .expect("write cdr2");

        // Both CDRs are written to the same hourly directory
        let dir1 = std::path::Path::new(&path1)
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let dir2 = std::path::Path::new(&path2)
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(dir1, dir2, "same-hour CDRs should land in same directory");

        // Both files should be distinct (different UUIDs)
        assert_ne!(path1, path2);
    }
}
