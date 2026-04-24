//! Placeholder. The stdin/stdout wrapper lands in a later task.
//! This file exists so `[[bin]] path = "src/main.rs"` resolves and
//! workspace-level `cargo test --workspace` does not fail with
//! "can't find bin" while the real binary is still being built up.

fn main() {
    unimplemented!("nixfleet-canonicalize: binary wrapper lands in a later task")
}
