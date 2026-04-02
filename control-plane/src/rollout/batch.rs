use nixfleet_types::rollout::RolloutStrategy;
use rand::seq::SliceRandom;

pub fn build_batches(machines: &[String], batch_sizes: &[String]) -> Vec<Vec<String>> {
    if machines.is_empty() {
        return vec![];
    }

    let mut rng = rand::rng();
    let mut remaining: Vec<String> = machines.to_vec();
    remaining.shuffle(&mut rng);

    let mut batches: Vec<Vec<String>> = Vec::new();
    let last_index = batch_sizes.len().saturating_sub(1);

    for (i, spec) in batch_sizes.iter().enumerate() {
        if remaining.is_empty() {
            break;
        }

        let take = if i == last_index {
            remaining.len()
        } else {
            parse_batch_size(spec, remaining.len())
        };

        let take = take.min(remaining.len());
        if take == 0 {
            continue;
        }

        let batch: Vec<String> = remaining.drain(..take).collect();
        batches.push(batch);
    }

    // If there are still remaining machines after all specs (shouldn't happen with last-spec rule)
    // but handle it defensively — append them to the last batch or as a new batch
    if !remaining.is_empty() {
        if let Some(last) = batches.last_mut() {
            last.extend(remaining);
        } else {
            batches.push(remaining);
        }
    }

    batches
}

fn parse_batch_size(spec: &str, remaining: usize) -> usize {
    if let Some(pct_str) = spec.strip_suffix('%') {
        let pct: f64 = pct_str.parse().unwrap_or(100.0);
        let count = (remaining as f64 * pct / 100.0).ceil() as usize;
        count.max(1)
    } else {
        spec.parse::<usize>().unwrap_or(1)
    }
}

pub fn effective_batch_sizes(
    strategy: &RolloutStrategy,
    batch_sizes: &Option<Vec<String>>,
) -> Vec<String> {
    match strategy {
        RolloutStrategy::Canary => vec!["1".to_string(), "100%".to_string()],
        RolloutStrategy::AllAtOnce => vec!["100%".to_string()],
        RolloutStrategy::Staged => batch_sizes
            .clone()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec!["100%".to_string()]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machines(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("web-{i:02}")).collect()
    }

    #[test]
    fn test_canary_batch_sizes() {
        let sizes = effective_batch_sizes(&nixfleet_types::rollout::RolloutStrategy::Canary, &None);
        assert_eq!(sizes, vec!["1", "100%"]);
    }

    #[test]
    fn test_build_batches_canary_20_machines() {
        let m = machines(20);
        let batches = build_batches(&m, &["1".to_string(), "100%".to_string()]);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[1].len(), 19);
    }

    #[test]
    fn test_build_batches_staged() {
        let m = machines(20);
        let batches = build_batches(
            &m,
            &["1".to_string(), "25%".to_string(), "100%".to_string()],
        );
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[1].len(), 5); // ceil(19 * 0.25) = 5
        assert_eq!(batches[2].len(), 14);
    }

    #[test]
    fn test_build_batches_all_at_once() {
        let m = machines(10);
        let batches = build_batches(&m, &["100%".to_string()]);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 10);
    }

    #[test]
    fn test_build_batches_single_machine() {
        let m = machines(1);
        let batches = build_batches(&m, &["1".to_string(), "100%".to_string()]);
        assert_eq!(batches.len(), 1); // second batch is empty, not created
        assert_eq!(batches[0].len(), 1);
    }

    #[test]
    fn test_build_batches_empty() {
        let batches = build_batches(&[], &["1".to_string()]);
        assert!(batches.is_empty());
    }
}
