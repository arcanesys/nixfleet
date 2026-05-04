//! Noun-based HTTP route modules.

pub(in crate::server) mod bootstrap_report;
pub(in crate::server) mod deferrals;
pub(in crate::server) mod enrollment;
pub(in crate::server) mod health;
pub(in crate::server) mod metrics;
pub(in crate::server) mod reports;
pub(in crate::server) mod rollouts;
pub(in crate::server) mod status;

/// Used by both `/v1/agent/report` and `/v1/agent/bootstrap-report` so a
/// single place defines the event-id shape ("evt-<ts_millis>-<rand>").
pub(in crate::server) fn new_event_id() -> String {
    format!(
        "evt-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        rand_suffix(8)
    )
}

fn rand_suffix(n: usize) -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..n)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}
