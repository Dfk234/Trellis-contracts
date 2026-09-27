//! User-facing recovery center for failed or pending operations.

use soroban_sdk::{contracttype, Address, Env, Symbol, Vec};

use crate::errors::Error;
use crate::storage::{persistent_get, persistent_set};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryStatus {
    Pending,
    Retried,
    Resolved,
    Cancelled,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryRecord {
    pub id: u64,
    pub actor: Address,
    pub operation: Symbol,
    pub status: RecoveryStatus,
    pub created_ledger: u32,
    pub updated_ledger: u32,
    pub error_code: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum RecoveryKey {
    NextId,
    Record(u64),
    Actor(Address),
}

pub fn open_recovery(
    env: &Env,
    actor: &Address,
    operation: &Symbol,
    error_code: u32,
) -> RecoveryRecord {
    let id: u64 = persistent_get(env, &RecoveryKey::NextId).unwrap_or(0);
    persistent_set(env, &RecoveryKey::NextId, &(id + 1));
    let record = RecoveryRecord {
        id,
        actor: actor.clone(),
        operation: operation.clone(),
        status: RecoveryStatus::Pending,
        created_ledger: env.ledger().sequence(),
        updated_ledger: env.ledger().sequence(),
        error_code,
    };
    persistent_set(env, &RecoveryKey::Record(id), &record);
    let mut ids: Vec<u64> =
        persistent_get(env, &RecoveryKey::Actor(actor.clone())).unwrap_or_else(|| Vec::new(env));
    ids.push_back(id);
    persistent_set(env, &RecoveryKey::Actor(actor.clone()), &ids);
    record
}

pub fn get_recovery(env: &Env, id: u64) -> Option<RecoveryRecord> {
    persistent_get(env, &RecoveryKey::Record(id))
}

pub fn list_recoveries(env: &Env, actor: &Address, limit: u32) -> Vec<RecoveryRecord> {
    let ids: Vec<u64> =
        persistent_get(env, &RecoveryKey::Actor(actor.clone())).unwrap_or_else(|| Vec::new(env));
    let mut out = Vec::new(env);
    let mut i = 0;
    while i < ids.len() && out.len() < limit {
        if let Some(record) = get_recovery(env, ids.get(i).unwrap_or(0)) {
            out.push_back(record);
        }
        i += 1;
    }
    out
}

pub fn update_recovery_status(
    env: &Env,
    id: u64,
    status: RecoveryStatus,
) -> Result<RecoveryRecord, Error> {
    let mut record: RecoveryRecord = get_recovery(env, id).ok_or(Error::NotFound)?;
    record.status = status;
    record.updated_ledger = env.ledger().sequence();
    persistent_set(env, &RecoveryKey::Record(id), &record);
    Ok(record)
}
