//! Operator subcommands of the `nixfleet` umbrella binary.
//!
//! Each module exposes:
//!   - `pub struct Args` — `#[derive(clap::Args)]` (not Parser) so it
//!     composes as a `clap::Subcommand` variant rather than running
//!     as its own top-level CLI.
//!   - `pub fn run(args: Args) -> anyhow::Result<()>` — the handler
//!     body, called from `bin/nixfleet.rs`'s dispatch match.
//!
//! Folded from former standalone binaries during v0.2 cleanup.
//! Submodule declarations are added by subsequent commits as each
//! binary is folded.

pub mod derive_pubkey;
pub mod mint_operator_cert;
