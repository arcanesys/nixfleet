//! Library surface for the nixfleet CLI.
//!
//! This crate is primarily a binary (`cli/src/main.rs`). A minimal library
//! target exists so integration tests in `cli/tests/` can exercise specific
//! helpers without shelling out to the compiled binary. Keep the surface
//! here as small as possible — if a test can run through the binary via
//! `assert_cmd`, prefer that.

pub mod config;
pub mod release;
