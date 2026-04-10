//! Library surface for the nixfleet CLI.
//!
//! This crate is primarily a binary (`cli/src/main.rs`). A minimal library
//! target exists so integration tests in `cli/tests/` can exercise specific
//! helpers without shelling out to the compiled binary. Keep the surface
//! here as small as possible — if a test can run through the binary via
//! `assert_cmd`, prefer that.
//!
//! Task 1 creates this file as an empty stub so the `[lib]` declaration
//! in `Cargo.toml` parses. Task 2 widens the release helpers and adds
//! `pub mod release;`. Task 16 adds `pub mod config;`.
