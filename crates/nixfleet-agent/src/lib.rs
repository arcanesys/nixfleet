#![allow(clippy::doc_lazy_continuation)]
//! NixFleet agent library: mTLS poll loop against the control plane,
//! realise + activate dispatched closures, confirm or report.

pub mod activation;
pub mod checkin_state;
pub mod comms;
pub mod compliance;
pub mod enrollment;
pub mod evidence_signer;
pub mod freshness;
pub mod health;
pub mod host_facts;
pub mod manifest_cache;
pub mod recovery;
