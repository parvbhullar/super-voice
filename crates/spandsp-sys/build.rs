use std::env;
use std::path::PathBuf;

fn main() {
    // Try SpanDSP 3.0 first, fall back to any available version (e.g. 0.0.6)
    let lib = pkg_config::Config::new()
        .atleast_version("3.0")
        .probe("spandsp")
        .or_else(|_| {
            // SpanDSP 3.0 may not be widely packaged; try without version constraint.
            // NOTE: API functions (dtmf_rx_*, echo_can_*) are stable since 0.0.6.
            println!(
                "cargo:warning=SpanDSP >=3.0 not found; \
                 trying any available version (0.0.6+ is supported)"
            );
            pkg_config::Config::new().probe("spandsp")
        })
        .unwrap_or_else(|e| {
            panic!(
                "SpanDSP not found: {e}\n\
                Install with:\n\
                  Debian/Ubuntu: apt-get install libspandsp-dev\n\
                  Homebrew:      brew install spandsp"
            )
        });

    // Emit link directives from pkg-config
    for path in &lib.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    for lib_name in &lib.libs {
        println!("cargo:rustc-link-lib={lib_name}");
    }

    let mut builder = bindgen::Builder::default()
        .header_contents("spandsp_wrapper.h", "#include <spandsp.h>\n")
        // Allowlist functions
        .allowlist_function("dtmf_rx")
        .allowlist_function("dtmf_rx_.*")
        .allowlist_function("super_tone_rx")
        .allowlist_function("echo_can_.*")
        .allowlist_function("super_tone_rx_.*")
        .allowlist_function("fax_.*")
        .allowlist_function("t38_terminal_.*")
        .allowlist_function("plc_.*")
        .allowlist_function("modem_connect_tones_.*")
        // Allowlist types
        .allowlist_type("dtmf_rx_state_t")
        .allowlist_type("echo_can_state_t")
        .allowlist_type("super_tone_rx_state_t")
        .allowlist_type("fax_state_t")
        .allowlist_type("t38_terminal_state_t")
        .allowlist_type("plc_state_t")
        // Suppress warnings for generated code via outer attribute
        .raw_line(
            "#[allow(non_upper_case_globals, non_camel_case_types, \
             non_snake_case, dead_code)]",
        );

    // Add include paths from pkg-config
    for include_path in &lib.include_paths {
        builder = builder.clang_arg(format!("-I{}", include_path.display()));
    }

    // Add Homebrew include path for libtiff (required by spandsp.h on macOS)
    if cfg!(target_os = "macos") {
        builder = builder.clang_arg("-I/opt/homebrew/include");
    }

    let bindings = builder
        .generate()
        .expect("Failed to generate SpanDSP bindings");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Failed to write SpanDSP bindings");
}
