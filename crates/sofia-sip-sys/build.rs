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

    let mut builder = bindgen::Builder::default()
        .header_contents(
            "sofia_sip_wrapper.h",
            "#include <sofia-sip/nua.h>\n\
             #include <sofia-sip/sdp.h>\n\
             #include <sofia-sip/su_root.h>\n\
             #include <sofia-sip/nta.h>\n\
             #include <sofia-sip/auth_module.h>\n",
        )
        // Allowlist functions
        .allowlist_function("nua_.*")
        .allowlist_function("su_root_.*")
        .allowlist_function("sip_.*")
        .allowlist_function("sdp_.*")
        .allowlist_function("nta_.*")
        .allowlist_function("tport_.*")
        // Allowlist types
        .allowlist_type("nua_.*")
        .allowlist_type("su_root_t")
        .allowlist_type("sip_.*")
        .allowlist_type("sdp_.*")
        .allowlist_type("nta_.*")
        .allowlist_type("tport_.*")
        .allowlist_type("msg_t")
        .allowlist_type("su_home_t")
        // Treat all types as opaque
        .opaque_type(".*")
        // Suppress warnings for generated code
        .raw_line("#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code)]");

    // Add include paths from pkg-config
    for include_path in &lib.include_paths {
        builder = builder.clang_arg(format!("-I{}", include_path.display()));
    }

    let bindings = builder
        .generate()
        .expect("Failed to generate Sofia-SIP bindings");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Failed to write Sofia-SIP bindings");
}
