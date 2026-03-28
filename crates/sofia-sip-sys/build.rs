use std::env;
use std::path::PathBuf;

fn main() {
    let lib = pkg_config::Config::new()
        .atleast_version("1.13.17")
        .probe("sofia-sip-ua")
        .or_else(|_| pkg_config::Config::new().probe("sofia-sip-ua"))
        .unwrap_or_else(|e| {
            panic!(
                "Sofia-SIP not found: {e}\n\
                Install with:\n\
                  Debian/Ubuntu: apt-get install libsofia-sip-ua-dev\n\
                  Homebrew:      brew install sofia-sip"
            )
        });

    // Emit link directives from pkg-config
    for path in &lib.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    for lib_name in &lib.libs {
        println!("cargo:rustc-link-lib={lib_name}");
    }

    // Build include path args first so the physical header can resolve includes.
    let include_args: Vec<String> = lib
        .include_paths
        .iter()
        .map(|p| format!("-I{}", p.display()))
        .collect();

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let wrapper_path = PathBuf::from(&manifest_dir).join("sofia_sip_wrapper.h");

    let mut builder = bindgen::Builder::default()
        .header(wrapper_path.to_str().unwrap())
        // Allowlist functions
        .allowlist_function("nua_.*")
        .allowlist_function("su_root_.*")
        .allowlist_function("sip_.*")
        .allowlist_function("sdp_.*")
        .allowlist_function("nta_.*")
        .allowlist_function("tport_.*")
        // Allowlist types and variables (tag system uses extern variables)
        .allowlist_type("nua_.*")
        .allowlist_type("su_root_t")
        .allowlist_type("su_duration_t")
        .allowlist_type("sip_.*")
        .allowlist_type("sdp_.*")
        .allowlist_type("nta_.*")
        .allowlist_type("tport_.*")
        .allowlist_type("tagi_t")
        .allowlist_type("tag_.*")
        .allowlist_type("msg_t")
        .allowlist_type("su_home_t")
        .allowlist_type("url_t")
        .allowlist_type("url_string_t")
        .allowlist_var("nutag_.*")
        .allowlist_var("tag_null.*")
        .allowlist_var("tag_end.*")
        // Treat all types as opaque
        .opaque_type(".*");
    // Note: lib.rs has #![allow(...)] at crate root covering the included bindings.

    // Add include paths from pkg-config
    for arg in &include_args {
        builder = builder.clang_arg(arg);
    }

    let bindings = builder
        .generate()
        .expect("Failed to generate Sofia-SIP bindings");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Failed to write Sofia-SIP bindings");
}
