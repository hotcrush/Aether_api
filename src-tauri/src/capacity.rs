use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub struct CapacityRegistry {
    active: Mutex<HashMap<String, i64>>,
}

impl CapacityRegistry {
    pub fn try_acquire(
        self: &Arc<Self>,
        account_id: &str,
        concurrency: i64,
    ) -> Option<CapacityLease> {
        let limit = concurrency.clamp(1, 1000);
        let mut active = self.active.lock().unwrap();
        let current = active.entry(account_id.to_string()).or_default();
        if *current >= limit {
            return None;
        }
        *current += 1;
        Some(CapacityLease {
            registry: Arc::clone(self),
            account_id: account_id.to_string(),
        })
    }

    pub fn snapshot(&self) -> HashMap<String, i64> {
        self.active.lock().unwrap().clone()
    }

    fn release(&self, account_id: &str) {
        let mut active = self.active.lock().unwrap();
        let Some(current) = active.get_mut(account_id) else {
            return;
        };
        *current -= 1;
        if *current <= 0 {
            active.remove(account_id);
        }
    }

    #[cfg(test)]
    fn current(&self, account_id: &str) -> i64 {
        self.active
            .lock()
            .unwrap()
            .get(account_id)
            .copied()
            .unwrap_or(0)
    }
}

#[derive(Debug)]
pub struct CapacityLease {
    registry: Arc<CapacityRegistry>,
    account_id: String,
}

impl Drop for CapacityLease {
    fn drop(&mut self) {
        self.registry.release(&self.account_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_enforces_limit_and_releases_on_drop() {
        let registry = Arc::new(CapacityRegistry::default());
        let first = registry.try_acquire("account", 2).unwrap();
        let second = registry.try_acquire("account", 2).unwrap();
        assert!(registry.try_acquire("account", 2).is_none());
        assert_eq!(registry.current("account"), 2);
        drop(first);
        assert_eq!(registry.current("account"), 1);
        drop(second);
        assert_eq!(registry.current("account"), 0);
    }
}
