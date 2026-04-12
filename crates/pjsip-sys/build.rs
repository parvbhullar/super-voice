// crates/pjsip-sys/build.rs
use std::env;
use std::path::PathBuf;

fn main() {
    // Probe pjproject via pkg-config (requires `bash scripts/install-pjproject.sh`).
    let pjsip = pkg_config::Config::new()
        .atleast_version("2.14")
        .probe("libpjproject")
        .unwrap_or_else(|e| {
            panic!(
                "pjproject not found: {e}\n\
                Install with: bash scripts/install-pjproject.sh"
            );
        });

    // Use link paths from pkg-config.
    for path in &pjsip.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }

    // We need only the SIP+SDP+lib stack — NOT pjmedia-audiodev/videodev/codec.
    // Linking pjsua2/pjsua/pjmedia-audiodev causes audio device initialization
    // on macOS which can block or trigger permission dialogs.
    //
    // Determine the platform suffix from the "pjsip-aarch64-..." library name.
    // The suffix is everything after "pjsip-" for the pjsip core library.
    // We look for a lib named exactly "pjsip-<suffix>" (not "pjsip-ua-<suffix>").
    let suffix = pjsip
        .libs
        .iter()
        .find(|l| {
            l.starts_with("pjsip-")
                && *l != "pjsip-ua"
                && !l.starts_with("pjsip-ua-")
                && *l != "pjsip-simple"
                && !l.starts_with("pjsip-simple-")
        })
        .map(|l| l.strip_prefix("pjsip-").unwrap_or("").to_string())
        .unwrap_or_default();

    // Only link the libraries we actually need for SIP B2BUA (no audio/video).
    // Order matters for static linking: most specific first.
    if !suffix.is_empty() {
        let needed_libs = [
            format!("pjsip-ua-{suffix}"),
            format!("pjsip-simple-{suffix}"),
            format!("pjsip-{suffix}"),
            format!("pjmedia-{suffix}"),   // SDP negotiation lives here
            format!("pjnath-{suffix}"),    // NAT traversal (needed by pjmedia)
            format!("pjlib-util-{suffix}"),
            format!("pj-{suffix}"),
        ];
        for lib in &needed_libs {
            println!("cargo:rustc-link-lib={lib}");
        }
    } else {
        // Fallback: link all pjproject libraries from pkg-config.
        for lib_name in &pjsip.libs {
            println!("cargo:rustc-link-lib={lib_name}");
        }
    }

    // pjproject is typically compiled with OpenSSL for TLS and digest auth.
    // Link OpenSSL via pkg-config to satisfy those references.
    if let Ok(ssl) = pkg_config::Config::new().probe("openssl") {
        for path in &ssl.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
        for lib_name in &ssl.libs {
            println!("cargo:rustc-link-lib={lib_name}");
        }
    } else {
        // Fallback: try the Homebrew openssl path on macOS.
        println!("cargo:rustc-link-search=native=/opt/homebrew/opt/openssl/lib");
        println!("cargo:rustc-link-lib=ssl");
        println!("cargo:rustc-link-lib=crypto");
    }

    // On macOS, pjproject uses CoreFoundation for UUID generation.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
    }

    // Build -I flags for clang from pkg-config include paths.
    let include_args: Vec<String> = pjsip
        .include_paths
        .iter()
        .map(|p| format!("-I{}", p.display()))
        .collect();

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let wrapper_path = PathBuf::from(&manifest_dir).join("pjsip_wrapper.h");

    // Detect target architecture for pjproject endianness declarations.
    // pjproject config.h requires explicit endianness on ARM/PowerPC.
    // On aarch64/arm64 (little-endian) we must pass these defines to clang.
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let is_arm = target_arch == "aarch64" || target_arch.starts_with("arm");

    let mut builder = bindgen::Builder::default()
        .header(wrapper_path.to_str().unwrap())
        // --- B2BUA-scoped allowlists ---
        // pjlib: init/shutdown, pool, thread, logging, string, error
        .allowlist_function("pj_init")
        .allowlist_function("pj_shutdown")
        .allowlist_function("pj_pool_create")
        .allowlist_function("pj_pool_release")
        .allowlist_function("pj_caching_pool_.*")
        .allowlist_function("pj_thread_.*")
        .allowlist_function("pj_log_.*")
        .allowlist_function("pj_str")
        .allowlist_function("pj_strerror")
        .allowlist_type("pj_pool_t")
        .allowlist_type("pj_pool_factory")
        .allowlist_type("pj_caching_pool")
        .allowlist_type("pj_str_t")
        .allowlist_type("pj_status_t")
        .allowlist_type("pj_bool_t")
        .allowlist_type("pj_thread_t")
        // pjsip endpoint
        .allowlist_function("pjsip_endpt_.*")
        .allowlist_type("pjsip_endpoint")
        .allowlist_type("pjsip_host_port")
        // pjsip transport (UDP/TCP/TLS)
        .allowlist_function("pjsip_udp_transport_start.*")
        .allowlist_function("pjsip_udp_transport_attach.*")
        .allowlist_function("pjsip_tcp_transport_start.*")
        .allowlist_function("pjsip_tls_transport_start.*")
        .allowlist_type("pjsip_transport.*")
        .allowlist_type("pjsip_tls_setting")
        .allowlist_type("pjsip_tpfactory")
        // pjsip transaction
        .allowlist_function("pjsip_tsx_.*")
        .allowlist_type("pjsip_transaction")
        // pjsip module registration
        .allowlist_function("pjsip_endpt_register_module")
        .allowlist_function("pjsip_endpt_unregister_module")
        .allowlist_type("pjsip_module")
        // pjsip message / header / URI
        .allowlist_type("pjsip_msg.*")
        .allowlist_type("pjsip_hdr.*")
        .allowlist_type("pjsip_generic_string_hdr")
        .allowlist_type("pjsip_uri")
        .allowlist_type("pjsip_sip_uri")
        .allowlist_type("pjsip_method.*")
        .allowlist_type("pjsip_status_code")
        .allowlist_type("pjsip_rx_data")
        .allowlist_type("pjsip_tx_data")
        .allowlist_function("pjsip_msg_.*")
        .allowlist_function("pjsip_hdr_.*")
        .allowlist_function("pjsip_generic_string_hdr_.*")
        .allowlist_function("pjsip_parse_uri")
        .allowlist_function("pjsip_uri_print")
        // pjsip-ua: UA layer (required for dialog/INVITE)
        .allowlist_function("pjsip_ua_init_module")
        .allowlist_function("pjsip_ua_instance")
        .allowlist_function("pjsip_ua_destroy")
        .allowlist_type("pjsip_user_agent")
        // pjsip-ua: dialog
        .allowlist_function("pjsip_dlg_.*")
        .allowlist_type("pjsip_dialog")
        .allowlist_type("pjsip_role_e")
        // pjsip-ua: INVITE session (RFC 3261 / 3262 / 3311)
        .allowlist_function("pjsip_inv_.*")
        .allowlist_type("pjsip_inv_session")
        .allowlist_type("pjsip_inv_state")
        .allowlist_type("pjsip_inv_callback")
        // pjsip-ua: Session Timers (RFC 4028)
        .allowlist_function("pjsip_timer_.*")
        .allowlist_type("pjsip_timer_setting")
        // pjsip-ua: 100rel / PRACK (RFC 3262)
        .allowlist_function("pjsip_100rel_.*")
        // pjsip-ua: Replaces (RFC 3891)
        .allowlist_function("pjsip_replaces_.*")
        .allowlist_type("pjsip_replaces_hdr")
        // pjsip-ua: REFER / transfer (RFC 3515)
        .allowlist_function("pjsip_xfer_.*")
        // pjsip-ua: registration client
        .allowlist_function("pjsip_regc_.*")
        .allowlist_type("pjsip_regc")
        // pjsip auth
        .allowlist_function("pjsip_auth_.*")
        .allowlist_type("pjsip_auth_clt_pref")
        .allowlist_type("pjsip_cred_info")
        // pjsip resolver (NAPTR/SRV — RFC 3263)
        .allowlist_function("pjsip_resolve")
        .allowlist_function("pjsip_endpt_resolve")
        .allowlist_type("pjsip_resolve_callback")
        .allowlist_type("pjsip_server_addresses")
        // pjlib-util DNS
        .allowlist_function("pj_dns_resolver_.*")
        .allowlist_type("pj_dns_resolver")
        // pjmedia SDP (offer/answer)
        .allowlist_function("pjmedia_sdp_.*")
        .allowlist_type("pjmedia_sdp_session")
        .allowlist_type("pjmedia_sdp_media")
        .allowlist_type("pjmedia_sdp_attr")
        .allowlist_type("pjmedia_sdp_conn")
        .allowlist_type("pjmedia_sdp_neg.*")
        // pjsip-simple: SUBSCRIBE / NOTIFY / presence
        .allowlist_function("pjsip_evsub_.*")
        .allowlist_type("pjsip_evsub.*")
        // CRITICAL: Do NOT use .opaque_type(".*")
        // Struct fields must be accessible for B2BUA use.
        .derive_debug(true)
        .derive_default(true);

    // Add include paths from pkg-config.
    for arg in &include_args {
        builder = builder.clang_arg(arg);
    }

    // pjproject config.h requires endianness to be declared on ARM/PowerPC.
    // ARM/aarch64 Macs are always little-endian.
    if is_arm {
        builder = builder
            .clang_arg("-DPJ_IS_LITTLE_ENDIAN=1")
            .clang_arg("-DPJ_IS_BIG_ENDIAN=0");
    }

    let bindings = builder
        .generate()
        .expect("Failed to generate pjsip bindings");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Failed to write pjsip bindings");
}
