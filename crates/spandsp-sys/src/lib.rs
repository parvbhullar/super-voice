#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

#[cfg(test)]
mod tests {
    /// Verify the crate compiles and the generated bindings module is accessible.
    ///
    /// Since all types are opaque, we just confirm the module loads without panic.
    #[test]
    fn bindings_accessible() {
        // If this compiles, the bindings were generated successfully.
        // Runtime verification requires a live SpanDSP installation.
    }
}
