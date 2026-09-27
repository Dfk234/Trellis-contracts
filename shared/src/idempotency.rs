//! Replay protection for retryable state-changing operations.
//!
//! Callers reserve a key before performing external effects and complete it
//! with the resulting operation identifier. Temporary storage gives keys an
//! explicit expiry without leaving unbounded replay records behind.

use soroban_sdk::{contracterror, contracttype, Bytes, BytesN, Env};

pub const DEFAULT_TTL_LEDGERS: u32 = 17_280;
pub const MAX_KEY_LEN: u32 = 64;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum IdempotencyError {
    EmptyKey = 1,
    KeyTooLong = 2,
    ConflictingRequest = 3,
    InProgress = 4,
    AlreadyCompleted = 5,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum Status {
    Pending,
    Completed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Record {
    pub request_hash: BytesN<32>,
    pub status: Status,
    pub result: Option<u64>,
}

fn storage_key(key: &Bytes) -> (u32, Bytes) {
    (0x4944, key.clone())
}

/// Reserve `key`, returning a completed record when the same request already
/// succeeded. A different request using the same key is always rejected.
pub fn begin(
    env: &Env,
    key: &Bytes,
    request_hash: &BytesN<32>,
    ttl: u32,
) -> Result<Option<Record>, IdempotencyError> {
    if key.len() == 0 {
        return Err(IdempotencyError::EmptyKey);
    }
    if key.len() > MAX_KEY_LEN {
        return Err(IdempotencyError::KeyTooLong);
    }
    let storage_key = storage_key(key);
    if let Some(record) = env.storage().temporary().get::<_, Record>(&storage_key) {
        if record.request_hash != *request_hash {
            return Err(IdempotencyError::ConflictingRequest);
        }
        return match record.status {
            Status::Pending => Err(IdempotencyError::InProgress),
            Status::Completed => Ok(Some(record)),
        };
    }
    let record = Record {
        request_hash: request_hash.clone(),
        status: Status::Pending,
        result: None,
    };
    env.storage().temporary().set(&storage_key, &record);
    env.storage().temporary().extend_ttl(
        &storage_key,
        ttl.min(DEFAULT_TTL_LEDGERS),
        ttl.max(1).min(DEFAULT_TTL_LEDGERS),
    );
    Ok(None)
}

/// Mark a reserved key complete and persist the operation result for retries.
pub fn complete(
    env: &Env,
    key: &Bytes,
    request_hash: &BytesN<32>,
    result: u64,
) -> Result<(), IdempotencyError> {
    let storage_key = storage_key(key);
    let mut record: Record = env
        .storage()
        .temporary()
        .get(&storage_key)
        .ok_or(IdempotencyError::EmptyKey)?;
    if record.request_hash != *request_hash {
        return Err(IdempotencyError::ConflictingRequest);
    }
    if record.status == Status::Completed {
        return Err(IdempotencyError::AlreadyCompleted);
    }
    record.status = Status::Completed;
    record.result = Some(result);
    env.storage().temporary().set(&storage_key, &record);
    Ok(())
}

