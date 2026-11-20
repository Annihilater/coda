//! Smart-Path LAN Direct <-> Smart Relay hot-switch backoff & flap-guard policy for `coda-desktop`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// Governs background LAN Direct upgrade probing while connected over Smart Relay:
/// - Exponential backoff (`15s -> 30s -> 60s -> 120s`) when LAN candidates are unreachable.
/// - Flap cooldown (`8s`) after a Direct P2P drop before attempting re-upgrade.
/// - Instant reset when manually triggered or when host endpoints update via account events.
#[derive(Debug)]
pub struct DesktopSmartPathPolicy {
    failures: HashMap<Uuid, u32>,
    last_drop_at: HashMap<Uuid, Instant>,
    base_interval: Duration,
    max_interval: Duration,
    drop_cooldown: Duration,
}

impl Default for DesktopSmartPathPolicy {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(15),
            Duration::from_secs(120),
            Duration::from_secs(8),
        )
    }
}

impl DesktopSmartPathPolicy {
    pub fn new(base_interval: Duration, max_interval: Duration, drop_cooldown: Duration) -> Self {
        Self {
            failures: HashMap::new(),
            last_drop_at: HashMap::new(),
            base_interval,
            max_interval,
            drop_cooldown,
        }
    }

    pub fn record_upgrade_success(&mut self, host_id: Uuid) {
        self.failures.insert(host_id, 0);
    }

    pub fn record_probe_failure(&mut self, host_id: Uuid) -> Duration {
        let fails = {
            let count = self.failures.entry(host_id).or_insert(0);
            *count = count.saturating_add(1);
            *count
        };
        self.backoff_for_failures(fails)
    }

    pub fn record_direct_drop(&mut self, host_id: Uuid, now: Instant) -> Duration {
        self.last_drop_at.insert(host_id, now);
        self.failures.insert(host_id, 1);
        self.base_interval.max(self.drop_cooldown)
    }

    pub fn reset_host(&mut self, host_id: Uuid) {
        self.failures.remove(&host_id);
    }

    pub fn reset_all(&mut self) {
        self.failures.clear();
    }

    pub fn can_probe_now(&self, host_id: Uuid, is_direct: bool, now: Instant) -> bool {
        if is_direct {
            return false;
        }
        match self.last_drop_at.get(&host_id) {
            None => true,
            Some(&dropped_at) => now.saturating_duration_since(dropped_at) >= self.drop_cooldown,
        }
    }

    pub fn next_delay(&self, host_id: Uuid) -> Duration {
        let count = self.failures.get(&host_id).copied().unwrap_or(0);
        if count == 0 {
            self.base_interval
        } else {
            self.backoff_for_failures(count)
        }
    }

    fn backoff_for_failures(&self, fails: u32) -> Duration {
        let shift = fails.saturating_sub(1).min(4);
        let mult = 1u32 << shift;
        self.base_interval
            .saturating_mul(mult)
            .min(self.max_interval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_path_policy_backoff_and_flap_guard() {
        let mut policy = DesktopSmartPathPolicy::new(
            Duration::from_secs(10),
            Duration::from_secs(60),
            Duration::from_secs(5),
        );
        let h = Uuid::new_v4();
        let t0 = Instant::now();

        assert!(policy.can_probe_now(h, false, t0));
        assert!(!policy.can_probe_now(h, true, t0));

        assert_eq!(policy.record_probe_failure(h), Duration::from_secs(10));
        assert_eq!(policy.record_probe_failure(h), Duration::from_secs(20));
        assert_eq!(policy.record_probe_failure(h), Duration::from_secs(40));
        assert_eq!(policy.record_probe_failure(h), Duration::from_secs(60));
        assert_eq!(policy.record_probe_failure(h), Duration::from_secs(60));

        policy.reset_host(h);
        assert_eq!(policy.next_delay(h), Duration::from_secs(10));

        policy.record_direct_drop(h, t0);
        assert!(!policy.can_probe_now(h, false, t0 + Duration::from_secs(2)));
        assert!(policy.can_probe_now(h, false, t0 + Duration::from_secs(5)));

        policy.record_upgrade_success(h);
        assert_eq!(policy.next_delay(h), Duration::from_secs(10));
    }
}
