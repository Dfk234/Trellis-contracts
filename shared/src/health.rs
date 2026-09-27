//! Dependency health checks for external services and network assumptions.

use soroban_sdk::{contracttype, Env, Symbol, Vec};

use crate::storage::{persistent_get, persistent_set};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DependencyStatus {
    Unknown,
    Healthy,
    Degraded,
    Down,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyHealth {
    pub name: Symbol,
    pub status: DependencyStatus,
    pub last_checked_ledger: u32,
    pub last_success_ledger: u32,
    pub failure_count: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum HealthKey {
    Names,
    Dependency(Symbol),
}

pub fn set_dependency_health(
    env: &Env,
    name: &Symbol,
    status: DependencyStatus,
) -> DependencyHealth {
    let previous = get_dependency_health(env, name).unwrap_or(DependencyHealth {
        name: name.clone(),
        status: DependencyStatus::Unknown,
        last_checked_ledger: 0,
        last_success_ledger: 0,
        failure_count: 0,
    });
    let failure_count = match status {
        DependencyStatus::Healthy => 0,
        DependencyStatus::Unknown => previous.failure_count,
        DependencyStatus::Degraded | DependencyStatus::Down => {
            previous.failure_count.saturating_add(1)
        }
    };
    let last_success_ledger = if status == DependencyStatus::Healthy {
        env.ledger().sequence()
    } else {
        previous.last_success_ledger
    };
    let health = DependencyHealth {
        name: name.clone(),
        status,
        last_checked_ledger: env.ledger().sequence(),
        last_success_ledger,
        failure_count,
    };
    persistent_set(env, &HealthKey::Dependency(name.clone()), &health);
    let mut names: Vec<Symbol> =
        persistent_get(env, &HealthKey::Names).unwrap_or_else(|| Vec::new(env));
    if !names.iter().any(|existing| existing == *name) {
        names.push_back(name.clone());
        persistent_set(env, &HealthKey::Names, &names);
    }
    health
}

pub fn get_dependency_health(env: &Env, name: &Symbol) -> Option<DependencyHealth> {
    persistent_get(env, &HealthKey::Dependency(name.clone()))
}

pub fn list_dependency_health(env: &Env) -> Vec<DependencyHealth> {
    let names: Vec<Symbol> =
        persistent_get(env, &HealthKey::Names).unwrap_or_else(|| Vec::new(env));
    let mut out = Vec::new(env);
    for name in names.iter() {
        if let Some(health) = get_dependency_health(env, &name) {
            out.push_back(health);
        }
    }
    out
}
