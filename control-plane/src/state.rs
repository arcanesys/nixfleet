use chrono::{DateTime, Utc};
use nixfleet_types::{DesiredGeneration, MachineLifecycle, Report};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Per-machine state held in memory for fast lookups.
#[derive(Debug, Clone)]
pub struct MachineState {
    pub desired_generation: Option<DesiredGeneration>,
    pub last_report: Option<Report>,
    pub last_seen: Option<DateTime<Utc>>,
    pub lifecycle: MachineLifecycle,
    pub registered_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
}

impl Default for MachineState {
    fn default() -> Self {
        Self::new()
    }
}

impl MachineState {
    pub fn new() -> Self {
        Self {
            desired_generation: None,
            last_report: None,
            last_seen: None,
            lifecycle: MachineLifecycle::Active,
            registered_at: None,
            tags: vec![],
        }
    }

    /// Create a new machine state in Pending lifecycle (for pre-registration).
    pub fn new_pending() -> Self {
        Self {
            desired_generation: None,
            last_report: None,
            last_seen: None,
            lifecycle: MachineLifecycle::Pending,
            registered_at: Some(Utc::now()),
            tags: vec![],
        }
    }
}

/// In-memory fleet state indexed by machine ID.
pub struct FleetState {
    pub machines: HashMap<String, MachineState>,
}

impl Default for FleetState {
    fn default() -> Self {
        Self::new()
    }
}

impl FleetState {
    pub fn new() -> Self {
        Self {
            machines: HashMap::new(),
        }
    }

    /// Get or create a machine entry.
    pub fn get_or_create(&mut self, machine_id: &str) -> &mut MachineState {
        self.machines.entry(machine_id.to_string()).or_default()
    }
}

/// Hydrate in-memory state from the database on startup.
pub async fn hydrate_from_db(
    state: &Arc<RwLock<FleetState>>,
    db: &crate::db::Db,
) -> anyhow::Result<()> {
    // Load registered machines with their lifecycle state
    let registered = db.list_machines()?;
    let mut fleet = state.write().await;
    for row in &registered {
        let machine = fleet.get_or_create(&row.machine_id);
        if let Some(lc) = MachineLifecycle::from_str_lc(&row.lifecycle) {
            machine.lifecycle = lc;
        }
    }

    // Load tags for each registered machine
    for row in &registered {
        let tags = db.get_machine_tags(&row.machine_id)?;
        let machine = fleet.get_or_create(&row.machine_id);
        machine.tags = tags;
    }

    // Load desired generations
    let generations = db.list_desired_generations()?;
    for (machine_id, hash) in generations {
        let machine = fleet.get_or_create(&machine_id);
        machine.desired_generation = Some(DesiredGeneration {
            hash,
            cache_url: None,
        });
    }
    let active_rollouts = db.list_rollouts_by_status(Some("running"), 100)?;
    let paused_rollouts = db.list_rollouts_by_status(Some("paused"), 100)?;

    tracing::info!(
        machines = fleet.machines.len(),
        active_rollouts = active_rollouts.len(),
        paused_rollouts = paused_rollouts.len(),
        "Hydrated fleet state from database"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fleet_state_new_is_empty() {
        let state = FleetState::new();
        assert!(state.machines.is_empty());
    }

    #[test]
    fn test_get_or_create_inserts_new() {
        let mut state = FleetState::new();
        let machine = state.get_or_create("web-01");
        assert!(machine.desired_generation.is_none());
        assert!(machine.last_report.is_none());
        assert_eq!(state.machines.len(), 1);
    }

    #[test]
    fn test_get_or_create_returns_existing() {
        let mut state = FleetState::new();
        state.get_or_create("web-01").desired_generation = Some(DesiredGeneration {
            hash: "/nix/store/abc123".to_string(),
            cache_url: None,
        });
        let machine = state.get_or_create("web-01");
        assert!(machine.desired_generation.is_some());
        assert_eq!(state.machines.len(), 1);
    }

    #[test]
    fn test_multiple_machines() {
        let mut state = FleetState::new();
        state.get_or_create("web-01");
        state.get_or_create("dev-01");
        state.get_or_create("mac-01");
        assert_eq!(state.machines.len(), 3);
    }
}
