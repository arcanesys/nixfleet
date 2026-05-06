//! Declarative key rotation: emit `Action::RotateTrustRoot` when a
//! trust slot's `retire_at` deadline has passed AND a `successor` is
//! declared. Pure function — informational only. The action signals
//! the operator's out-of-band tooling to rotate
//! `current → previous, successor → current` in the next fleet
//! commit. The CP NEVER self-mutates trust roots; that's the
//! point of the v0.2 inversion-of-trust property.
//!
//! Closes nixfleet#63.

use chrono::{DateTime, Utc};
use nixfleet_proto::trust::{KeySlot, TrustConfig};

use crate::action::Action;

/// Returns one `Action::RotateTrustRoot` per trust slot whose
/// `retire_at <= now` AND `successor.is_some()`. Each slot is
/// checked independently — both `ciReleaseKey` and `orgRootKey`
/// can carry rotation metadata.
///
/// Idempotent: emits the same action every tick until the operator
/// rotates the slot in fleet.nix (after which `successor` clears
/// from the new fleet.resolved + trust.json, and the predicate stops
/// firing).
///
/// Tickrate-friendly: no DB writes, no I/O, just `now < retire_at`
/// arithmetic. Safe to call from the reconcile loop's hot path.
pub fn check_trust_rotations(trust: &TrustConfig, now: DateTime<Utc>) -> Vec<Action> {
    let mut out = Vec::new();
    if let Some(retire_at) = is_rotation_due(&trust.ci_release_key, now) {
        out.push(Action::RotateTrustRoot {
            which: "ciReleaseKey".to_string(),
            retire_at,
        });
    }
    if let Some(org_root) = trust.org_root_key.as_ref() {
        if let Some(retire_at) = is_rotation_due(org_root, now) {
            out.push(Action::RotateTrustRoot {
                which: "orgRootKey".to_string(),
                retire_at,
            });
        }
    }
    out
}

/// Returns `Some(retire_at)` when this slot's rotation is due,
/// `None` otherwise. Encapsulates the predicate so the rotation-due
/// check stays in one place: same field-pair as `active_keys_at`,
/// just opposite sense.
fn is_rotation_due(slot: &KeySlot, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let retire_at = slot.retire_at?;
    slot.successor.as_ref()?;
    if now < retire_at {
        return None;
    }
    Some(retire_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nixfleet_proto::trust::TrustedPubkey;

    fn key(public: &str) -> TrustedPubkey {
        TrustedPubkey {
            algorithm: "ed25519".into(),
            public: public.into(),
        }
    }

    fn slot_with(
        successor: Option<TrustedPubkey>,
        retire_at: Option<DateTime<Utc>>,
    ) -> KeySlot {
        KeySlot {
            current: Some(key("AAAA")),
            previous: None,
            reject_before: None,
            successor,
            retire_at,
        }
    }

    fn trust_with(ci: KeySlot, org: Option<KeySlot>) -> TrustConfig {
        TrustConfig {
            schema_version: TrustConfig::CURRENT_SCHEMA_VERSION,
            ci_release_key: ci,
            cache_keys: vec![],
            org_root_key: org,
            root_ca_pem: None,
            issuance_ca_pems: vec![],
        }
    }

    #[test]
    fn pre_announce_window_emits_nothing() {
        // retire_at is in the future → still within overlap, no rotation due.
        let now = Utc::now();
        let slot = slot_with(Some(key("CCCC")), Some(now + chrono::Duration::days(7)));
        let actions = check_trust_rotations(&trust_with(slot, None), now);
        assert!(actions.is_empty());
    }

    #[test]
    fn post_retire_with_successor_emits_rotate_for_ci_release_key() {
        // retire_at is past → rotation due.
        let now = Utc::now();
        let retire_at = now - chrono::Duration::hours(1);
        let slot = slot_with(Some(key("CCCC")), Some(retire_at));
        let actions = check_trust_rotations(&trust_with(slot, None), now);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::RotateTrustRoot { which, retire_at: r } => {
                assert_eq!(which, "ciReleaseKey");
                assert_eq!(*r, retire_at);
            }
            other => panic!("expected RotateTrustRoot, got {other:?}"),
        }
    }

    #[test]
    fn post_retire_without_successor_emits_nothing() {
        // retire_at past but no successor declared → operator hasn't
        // staged a rotation; nothing to signal.
        let now = Utc::now();
        let slot = slot_with(None, Some(now - chrono::Duration::days(1)));
        let actions = check_trust_rotations(&trust_with(slot, None), now);
        assert!(actions.is_empty());
    }

    #[test]
    fn successor_without_retire_at_emits_nothing() {
        // No deadline → can't compute "past deadline". The Nix
        // schema asserts paired-options, so this is unreachable from
        // the operator path; this test pins runtime behaviour for
        // malformed trust.json.
        let now = Utc::now();
        let slot = slot_with(Some(key("CCCC")), None);
        let actions = check_trust_rotations(&trust_with(slot, None), now);
        assert!(actions.is_empty());
    }

    #[test]
    fn org_root_key_rotation_also_signaled() {
        // orgRootKey is its own slot; rotation tracked separately.
        let now = Utc::now();
        let retire_at = now - chrono::Duration::minutes(30);
        let ci_slot = slot_with(None, None); // ciReleaseKey: not due
        let org_slot = slot_with(Some(key("DDDD")), Some(retire_at));
        let actions = check_trust_rotations(&trust_with(ci_slot, Some(org_slot)), now);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::RotateTrustRoot { which, .. } => assert_eq!(which, "orgRootKey"),
            other => panic!("expected RotateTrustRoot orgRootKey, got {other:?}"),
        }
    }

    #[test]
    fn both_slots_due_simultaneously_emits_two_actions() {
        let now = Utc::now();
        let retire_at = now - chrono::Duration::hours(1);
        let ci_slot = slot_with(Some(key("CCCC")), Some(retire_at));
        let org_slot = slot_with(Some(key("DDDD")), Some(retire_at));
        let actions = check_trust_rotations(&trust_with(ci_slot, Some(org_slot)), now);
        assert_eq!(actions.len(), 2);
        let whiches: Vec<&str> = actions
            .iter()
            .map(|a| match a {
                Action::RotateTrustRoot { which, .. } => which.as_str(),
                _ => panic!("expected RotateTrustRoot"),
            })
            .collect();
        assert!(whiches.contains(&"ciReleaseKey"));
        assert!(whiches.contains(&"orgRootKey"));
    }

    #[test]
    fn exactly_at_deadline_is_rotation_due() {
        // `now >= retire_at` per spec — equality is the moment of rotation,
        // not the last instant of overlap.
        let now = Utc::now();
        let slot = slot_with(Some(key("CCCC")), Some(now));
        let actions = check_trust_rotations(&trust_with(slot, None), now);
        assert_eq!(actions.len(), 1);
    }
}
