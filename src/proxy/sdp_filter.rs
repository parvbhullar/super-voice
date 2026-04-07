use crate::media::negotiate::parse_rtpmap;
use crate::redis_state::types::MediaConfig;

/// Map a `CodecType` to its canonical lowercase name used in trunk config.
fn codec_type_name(ct: &audio_codec::CodecType) -> &'static str {
    match ct {
        audio_codec::CodecType::PCMU => "pcmu",
        audio_codec::CodecType::PCMA => "pcma",
        audio_codec::CodecType::G722 => "g722",
        audio_codec::CodecType::G729 => "g729",
        audio_codec::CodecType::Opus => "opus",
        audio_codec::CodecType::TelephoneEvent => "telephone-event",
    }
}

/// Static payload-type to codec-name mapping (RFC 3551).
fn static_pt_name(pt: u8) -> Option<&'static str> {
    match pt {
        0 => Some("pcmu"),
        8 => Some("pcma"),
        9 => Some("g722"),
        18 => Some("g729"),
        _ => None,
    }
}

/// Resolve the effective allowed-codec list for a trunk.
///
/// Prefers `media.codecs` when present and non-empty, falls back to the
/// legacy top-level `codecs` field.  Returns `None` when neither is set
/// (meaning "allow all codecs").
pub fn resolve_trunk_codecs(
    media: &Option<MediaConfig>,
    codecs: &Option<Vec<String>>,
) -> Option<Vec<String>> {
    if let Some(mc) = media {
        if !mc.codecs.is_empty() {
            return Some(mc.codecs.clone());
        }
    }
    if let Some(c) = codecs {
        if !c.is_empty() {
            return Some(c.clone());
        }
    }
    None
}

/// Filter an SDP offer to retain only codecs present in `allowed`.
///
/// * Parses `a=rtpmap:` lines via [`parse_rtpmap`] to build a PT→name map.
/// * Keeps PTs whose codec name (lowercase) appears in `allowed` (also
///   lowercased).
/// * Always preserves `telephone-event` PTs regardless of `allowed`.
/// * Handles well-known static PTs (0=pcmu, 8=pcma, 9=g722, 18=g729).
/// * Rewrites the `m=audio` line with surviving PTs only.
/// * Strips `a=rtpmap:` / `a=fmtp:` lines for removed PTs.
/// * Returns `Err` with a descriptive message when zero non-telephone-event
///   audio codecs survive (suitable for a SIP 488 response).
pub fn filter_sdp_codecs(sdp: &str, allowed: &[String]) -> Result<String, String> {
    let allowed_lower: Vec<String> = allowed.iter().map(|s| s.to_lowercase()).collect();

    // --- First pass: discover PT→codec-name via rtpmap lines ---------------
    let mut pt_name: std::collections::HashMap<u8, String> = std::collections::HashMap::new();

    for line in sdp.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed
            .strip_prefix("a=rtpmap:")
            .or_else(|| trimmed.strip_prefix("a=rtpmap: "))
        {
            if let Ok((pt, ct, _clock, _ch)) = parse_rtpmap(val) {
                pt_name.insert(pt, codec_type_name(&ct).to_string());
            }
        }
    }

    // --- Decide which PTs survive ------------------------------------------
    let mut keep_pts: std::collections::HashSet<u8> = std::collections::HashSet::new();

    // Check rtpmap-discovered PTs
    for (&pt, name) in &pt_name {
        let name_lower = name.to_lowercase();
        if name_lower == "telephone-event" || allowed_lower.contains(&name_lower) {
            keep_pts.insert(pt);
        }
    }

    // Check static PTs that may appear in m= line without an rtpmap
    // (we'll filter the m= line formats below and need to know which statics
    // to keep).

    // --- Rewrite SDP -------------------------------------------------------
    let mut out_lines: Vec<String> = Vec::new();
    let mut audio_codec_count: usize = 0;

    for line in sdp.lines() {
        let trimmed = line.trim();

        // ---- m=audio line -------------------------------------------------
        if trimmed.starts_with("m=audio ") {
            let parts: Vec<&str> = trimmed.splitn(4, ' ').collect();
            if parts.len() >= 4 {
                // parts: ["m=audio", port, proto, "0 8 9 101 ..."]
                let port = parts[1];
                let proto = parts[2];
                let formats_str = parts[3];

                let mut surviving_fmts: Vec<String> = Vec::new();
                for fmt in formats_str.split_whitespace() {
                    if let Ok(pt) = fmt.parse::<u8>() {
                        // Already mapped via rtpmap?
                        if keep_pts.contains(&pt) {
                            surviving_fmts.push(fmt.to_string());
                            continue;
                        }
                        // Static PT without explicit rtpmap
                        if !pt_name.contains_key(&pt) {
                            if let Some(sname) = static_pt_name(pt) {
                                if allowed_lower.contains(&sname.to_string()) {
                                    keep_pts.insert(pt);
                                    surviving_fmts.push(fmt.to_string());
                                    continue;
                                }
                            }
                        }
                        // PT not allowed – drop
                    } else {
                        // Non-numeric format token (unlikely but preserve)
                        surviving_fmts.push(fmt.to_string());
                    }
                }

                // Count non-telephone-event audio codecs
                for fmt in &surviving_fmts {
                    if let Ok(pt) = fmt.parse::<u8>() {
                        let name = pt_name
                            .get(&pt)
                            .map(|s| s.as_str())
                            .or_else(|| static_pt_name(pt));
                        if let Some(n) = name {
                            if n != "telephone-event" {
                                audio_codec_count += 1;
                            }
                        }
                    }
                }

                if audio_codec_count == 0 {
                    return Err(format!(
                        "no codec overlap: allowed=[{}] vs offer PTs",
                        allowed.join(",")
                    ));
                }

                out_lines.push(format!(
                    "m=audio {} {} {}",
                    port,
                    proto,
                    surviving_fmts.join(" ")
                ));
                continue;
            }
        }

        // ---- a=rtpmap: / a=fmtp: for removed PTs -------------------------
        if trimmed.starts_with("a=rtpmap:") || trimmed.starts_with("a=fmtp:") {
            // Extract the PT number (first token after the colon)
            let after_colon = if trimmed.starts_with("a=rtpmap:") {
                &trimmed["a=rtpmap:".len()..]
            } else {
                &trimmed["a=fmtp:".len()..]
            };
            if let Some(pt_str) = after_colon.split_whitespace().next() {
                if let Ok(pt) = pt_str.parse::<u8>() {
                    if !keep_pts.contains(&pt) {
                        continue; // strip this line
                    }
                }
            }
        }

        // ---- Everything else: preserve ------------------------------------
        out_lines.push(line.to_string());
    }

    Ok(out_lines.join("\r\n") + "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Typical multi-codec offer: PCMU(0), PCMA(8), opus(111), telephone-event(101).
    fn sample_offer() -> String {
        [
            "v=0",
            "o=- 123 123 IN IP4 10.0.0.1",
            "s=-",
            "c=IN IP4 10.0.0.1",
            "t=0 0",
            "m=audio 20000 RTP/AVP 0 8 111 101",
            "a=rtpmap:0 PCMU/8000",
            "a=rtpmap:8 PCMA/8000",
            "a=rtpmap:111 opus/48000/2",
            "a=fmtp:111 minptime=10;useinbandfec=1",
            "a=rtpmap:101 telephone-event/8000",
            "a=fmtp:101 0-16",
            "a=ptime:20",
        ]
        .join("\n")
    }

    #[test]
    fn filter_keeps_only_pcmu() {
        let allowed = vec!["pcmu".to_string()];
        let result = filter_sdp_codecs(&sample_offer(), &allowed).unwrap();
        // m= line should only list 0 and 101
        assert!(result.contains("m=audio 20000 RTP/AVP 0 101"));
        // PCMU rtpmap kept
        assert!(result.contains("a=rtpmap:0 PCMU/8000"));
        // PCMA stripped
        assert!(!result.contains("a=rtpmap:8 PCMA/8000"));
        // opus stripped
        assert!(!result.contains("a=rtpmap:111 opus/48000/2"));
        assert!(!result.contains("a=fmtp:111"));
        // telephone-event kept
        assert!(result.contains("a=rtpmap:101 telephone-event/8000"));
        assert!(result.contains("a=fmtp:101 0-16"));
    }

    #[test]
    fn filter_allows_multiple_codecs() {
        let allowed = vec!["pcmu".to_string(), "pcma".to_string()];
        let result = filter_sdp_codecs(&sample_offer(), &allowed).unwrap();
        assert!(result.contains("m=audio 20000 RTP/AVP 0 8 101"));
        assert!(result.contains("a=rtpmap:0 PCMU/8000"));
        assert!(result.contains("a=rtpmap:8 PCMA/8000"));
        assert!(!result.contains("a=rtpmap:111"));
    }

    #[test]
    fn error_on_no_overlap() {
        let allowed = vec!["g729".to_string()];
        let err = filter_sdp_codecs(&sample_offer(), &allowed).unwrap_err();
        assert!(err.starts_with("no codec overlap:"));
    }

    #[test]
    fn telephone_event_preserved_even_when_not_in_allowed() {
        let allowed = vec!["pcmu".to_string()];
        let result = filter_sdp_codecs(&sample_offer(), &allowed).unwrap();
        assert!(result.contains("a=rtpmap:101 telephone-event/8000"));
        assert!(result.contains("a=fmtp:101 0-16"));
        // telephone-event PT in m= line
        assert!(result.contains(" 101"));
    }

    #[test]
    fn non_audio_sdp_lines_preserved() {
        let allowed = vec!["pcmu".to_string()];
        let result = filter_sdp_codecs(&sample_offer(), &allowed).unwrap();
        assert!(result.contains("v=0"));
        assert!(result.contains("o=- 123 123 IN IP4 10.0.0.1"));
        assert!(result.contains("c=IN IP4 10.0.0.1"));
        assert!(result.contains("a=ptime:20"));
    }

    #[test]
    fn opus_only_strips_g711() {
        let allowed = vec!["opus".to_string()];
        let result = filter_sdp_codecs(&sample_offer(), &allowed).unwrap();
        assert!(result.contains("m=audio 20000 RTP/AVP 111 101"));
        assert!(!result.contains("a=rtpmap:0 PCMU"));
        assert!(!result.contains("a=rtpmap:8 PCMA"));
        assert!(result.contains("a=rtpmap:111 opus/48000/2"));
        assert!(result.contains("a=fmtp:111"));
    }

    #[test]
    fn resolve_prefers_media_codecs() {
        let media = Some(MediaConfig {
            codecs: vec!["opus".to_string()],
            dtmf_mode: "rfc2833".to_string(),
            srtp: None,
            media_mode: None,
        });
        let legacy = Some(vec!["pcmu".to_string()]);
        let result = resolve_trunk_codecs(&media, &legacy);
        assert_eq!(result, Some(vec!["opus".to_string()]));
    }

    #[test]
    fn resolve_falls_back_to_legacy() {
        let media: Option<MediaConfig> = None;
        let legacy = Some(vec!["pcma".to_string()]);
        let result = resolve_trunk_codecs(&media, &legacy);
        assert_eq!(result, Some(vec!["pcma".to_string()]));
    }

    #[test]
    fn resolve_returns_none_when_nothing_configured() {
        let result = resolve_trunk_codecs(&None, &None);
        assert!(result.is_none());
    }

    #[test]
    fn resolve_returns_none_for_empty_media_codecs() {
        let media = Some(MediaConfig {
            codecs: vec![],
            dtmf_mode: "rfc2833".to_string(),
            srtp: None,
            media_mode: None,
        });
        let result = resolve_trunk_codecs(&media, &None);
        assert!(result.is_none());
    }

    #[test]
    fn output_uses_crlf() {
        let allowed = vec!["pcmu".to_string()];
        let result = filter_sdp_codecs(&sample_offer(), &allowed).unwrap();
        // Every line boundary should be \r\n
        assert!(result.contains("\r\n"));
        // No bare \n without preceding \r
        let without_crlf = result.replace("\r\n", "");
        assert!(
            !without_crlf.contains('\n'),
            "bare LF found outside CRLF pairs"
        );
    }
}
