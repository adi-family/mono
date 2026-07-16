//! Employee Registry
//!
//! Core primitive that tracks which WASM instances are running and what identity
//! each one claims via `sdk.register(...)`. Every loop started by a WASM inherits
//! the employee identity stored here.
//!
//! Consumers (e.g. orchestration's MessageEmployee) query this registry instead
//! of maintaining their own employee lists.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmployeeRegistration {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// Process-wide registry of employees keyed by their registered name.
#[derive(Debug, Default)]
pub struct EmployeeRegistry {
    inner: Mutex<HashMap<String, Arc<EmployeeRegistration>>>,
}

impl EmployeeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a new registration. Errors if `name` is already taken.
    pub fn insert(&self, reg: EmployeeRegistration) -> Result<Arc<EmployeeRegistration>, String> {
        let mut map = self.inner.lock().unwrap();
        if map.contains_key(&reg.name) {
            return Err(format!("employee '{}' already registered", reg.name));
        }
        let arc = Arc::new(reg);
        map.insert(arc.name.clone(), arc.clone());
        Ok(arc)
    }

    /// Remove a registration (called when a WASM instance shuts down).
    pub fn remove(&self, name: &str) {
        self.inner.lock().unwrap().remove(name);
    }

    pub fn get(&self, name: &str) -> Option<Arc<EmployeeRegistration>> {
        self.inner.lock().unwrap().get(name).cloned()
    }

    pub fn list(&self) -> Vec<Arc<EmployeeRegistration>> {
        self.inner.lock().unwrap().values().cloned().collect()
    }

    /// List employees whose labels match every `(key, value)` pair in `filter`.
    pub fn list_matching(
        &self,
        filter: &HashMap<String, String>,
    ) -> Vec<Arc<EmployeeRegistration>> {
        self.inner
            .lock()
            .unwrap()
            .values()
            .filter(|r| filter.iter().all(|(k, v)| r.labels.get(k) == Some(v)))
            .cloned()
            .collect()
    }
}
