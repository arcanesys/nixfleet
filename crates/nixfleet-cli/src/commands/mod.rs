//! Operator subcommands of the `nixfleet` umbrella binary. Each module
//! exposes a `pub struct Args` (derived from `clap::Args`, not Parser) and a
//! `pub fn run(args: Args) -> Result<()>` handler.

pub mod derive_pubkey;
pub mod mint_operator_cert;
pub mod mint_token;
