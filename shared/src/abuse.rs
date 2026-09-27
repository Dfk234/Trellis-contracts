//! Abuse controls layered on top of quota checks.
//!
//! Quotas limit repeated use of a resource. Abuse scoring records why an
//! operation was rejected so frontends and maintainers can distinguish normal
//! throttling from suspicious patterns.

use soroban_sdk::{contracttype, Address, Env, Symbol};

use crate::errors::Error;
use crate::storage::{persistent_get, persistent_set};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbusePolicy {
    pub max_score: u32,
    pub decay_per_window: u32,
    pub window_ledgers: u32,
}

impl Default for AbusePolicy {
    fn default() -> Self {
        Self {
            max_score: 100,
            decay_per_window: 10,
            window_ledgers: 17_280,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbuseState {
    pub score: u32,
    pub last_ledger: u32,
    pub strikes: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum AbuseKey {
    Policy(Symbol),
    State(Address, Symbol),
}

pub fn set_abuse_policy(env: &Env, resource: &Symbol, policy: &AbusePolicy) -> Result<(), Error> {
    if policy.max_score == 0 || policy.window_ledgers == 0 {
        return Err(Error::ConfigInvalid);
    }
    persistent_set(env, &AbuseKey::Policy(resource.clone()), policy);
    Ok(())
}

pub fn get_abuse_policy(env: &Env, resource: &Symbol) -> AbusePolicy {
    persistent_get(env, &AbuseKey::Policy(resource.clone())).unwrap_or_default()
}

pub fn get_abuse_state(env: &Env, actor: &Address, resource: &Symbol) -> AbuseState {
    persistent_get(env, &AbuseKey::State(actor.clone(), resource.clone())).unwrap_or(AbuseState {
        score: 0,
        last_ledger: env.ledger().sequence(),
        strikes: 0,
    })
}

pub fn record_abuse(
    env: &Env,
    actor: &Address,
    resource: &Symbol,
    weight: u32,
) -> Result<AbuseState, Error> {
    let policy = get_abuse_policy(env, resource);
    let mut state = get_abuse_state(env, actor, resource);
    let elapsed = env.ledger().sequence().saturating_sub(state.last_ledger);
    if elapsed >= policy.window_ledgers {
        let windows = elapsed / policy.window_ledgers;
        state.score = state
            .score
            .saturating_sub(policy.decay_per_window.saturating_mul(windows));
    }
    state.score = state.score.saturating_add(weight);
    state.strikes = state.strikes.saturating_add(1);
    state.last_ledger = env.ledger().sequence();
    persistent_set(
        env,
        &AbuseKey::State(actor.clone(), resource.clone()),
        &state,
    );
    if state.score > policy.max_score {
        return Err(Error::QuotaExceeded);
    }
    Ok(state)
}
