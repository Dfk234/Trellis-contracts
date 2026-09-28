#![no_std]
//! # Oracle Contract
//!
//! Off-chain data integration and verification bridge for Trellis.
//!
//! ## Overview
//!
//! The Oracle Contract acts as a bridge between off-chain verification systems and on-chain state.
//! It stores references to:
//! - **AI verification proofs**: Hash references to external verification
//! - **Metadata pointers**: URLs or identifiers for off-chain data
//! - **Signatures**: External signatures proving authenticity
//!
//! This contract does not execute the verification logic itself, but rather anchors
//! cryptographic proof that verification occurred.
//!
//! ## Example Flow
//!
//! 1. Off-chain system verifies user identity via AI or document review
//! 2. System generates a proof hash and calls the oracle contract
//! 3. On-chain applications query the oracle to confirm verification
//! 4. Applications can trust that a specific wallet has been verified
//!
//! ## Queries
//!
//! - Retrieve verification metadata for a wallet
//! - Check proof hashes and signatures
//!
//! For full API details, see the module items below.

mod errors;
mod events;
mod storage;
mod types;

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Bytes, Env, Symbol};

use errors::OracleError;
use shared::events::emit_module_initialized;
use shared::{record_action_audit_event, ResourceLink, TimelineEventType};
use types::{FeedLatest, PriceSubmission};

const MAX_FUTURE_SKEW_SECS: u64 = 60;
const MIN_PRICE_QUORUM: u32 = 2;

#[cfg(test)]
mod tests;

#[contract]
pub struct OracleContract;

#[contractimpl]
impl OracleContract {
    /// Initialize the oracle contract with an admin.
    pub fn initialize(env: Env, admin: Address) -> Result<(), OracleError> {
        shared::auth::initialize_admin(&env, &admin).map_err(|error| match error {
            shared::Error::AlreadyInitialized => OracleError::AlreadyInitialized,
            _ => OracleError::Unauthorized,
        })?;
        emit_module_initialized(
            &env,
            symbol_short!("oracle"),
            1,
            &admin,
            env.ledger().timestamp(),
        );
        Ok(())
    }

    /// Register a new authorized submitter (admin only).
    pub fn register_submitter(
        env: Env,
        caller: Address,
        submitter: Address,
    ) -> Result<(), OracleError> {
        shared::auth::require_admin(&env, &caller).map_err(|_| OracleError::Unauthorized)?;
        let was_active = storage::is_submitter_active(&env, &submitter);
        storage::register_submitter(&env, submitter.clone(), env.ledger().timestamp())?;
        record_action_audit_event(
            &env,
            &caller,
            TimelineEventType::RoleChanged,
            ResourceLink {
                kind: Bytes::from_slice(&env, b"oracle"),
                id: 0,
                revision: 0,
            },
            symbol_short!("oracle"),
            symbol_short!("signer"),
            symbol_short!("adm_grant"),
            Some(submitter.clone()),
            Some(symbol_short!("signer")),
            Some(if was_active { 1 } else { 0 }),
            Some(1),
        )
        .map_err(|_| OracleError::InternalError)?;
        events::emit_submitter_registered(&env, &submitter, env.ledger().timestamp());
        Ok(())
    }

    /// Deactivate an authorized submitter (admin only).
    pub fn deactivate_submitter(
        env: Env,
        caller: Address,
        submitter: Address,
    ) -> Result<(), OracleError> {
        shared::auth::require_admin(&env, &caller).map_err(|_| OracleError::Unauthorized)?;
        let was_active = storage::is_submitter_active(&env, &submitter);
        storage::deactivate_submitter(&env, &submitter);
        record_action_audit_event(
            &env,
            &caller,
            TimelineEventType::RoleChanged,
            ResourceLink {
                kind: Bytes::from_slice(&env, b"oracle"),
                id: 0,
                revision: 0,
            },
            symbol_short!("oracle"),
            symbol_short!("signer"),
            symbol_short!("adm_rvok"),
            Some(submitter.clone()),
            Some(symbol_short!("signer")),
            Some(if was_active { 1 } else { 0 }),
            Some(0),
        )
        .map_err(|_| OracleError::InternalError)?;
        events::emit_submitter_deactivated(&env, &submitter, env.ledger().timestamp());
        Ok(())
    }

    /// Submit a price for a feed (authorized submitters only).
    ///
    /// Includes replay protection (nonce-based) and staleness validation.
    pub fn submit_price(
        env: Env,
        submitter: Address,
        feed_id: Symbol,
        price: i128,
        decimals: u32,
        timestamp: u64,
        nonce: u64,
    ) -> Result<u64, OracleError> {
        submitter.require_auth();

        // Validate price and decimals
        if price < 0 {
            return Err(OracleError::InvalidPrice);
        }
        if decimals > 18 {
            return Err(OracleError::InvalidDecimals);
        }

        // Check submitter is authorized
        if !storage::is_submitter_active(&env, &submitter) {
            return Err(OracleError::SubmitterNotAuthorized);
        }

        // Replay protection: check nonce
        let expected_nonce = storage::get_nonce(&env, &submitter, &feed_id);
        if nonce != expected_nonce + 1 {
            return Err(OracleError::DuplicateSubmission);
        }

        // Staleness check: ensure timestamp is neither stale nor too far ahead.
        let current_time = env.ledger().timestamp();
        let staleness_window = storage::get_staleness_window(&env);
        if timestamp > current_time.saturating_add(MAX_FUTURE_SKEW_SECS) {
            return Err(OracleError::SubmissionFromFuture);
        }
        let expires_at = timestamp
            .checked_add(staleness_window)
            .ok_or(OracleError::SubmissionStale)?;
        if current_time > expires_at {
            return Err(OracleError::SubmissionStale);
        }

        // Generate submission
        let submission_id = storage::next_submission_id(&env)?;
        let submission = PriceSubmission {
            id: submission_id,
            submitter: submitter.clone(),
            feed_id: feed_id.clone(),
            price,
            decimals,
            timestamp,
            nonce,
        };

        // Store submission
        storage::append_submission(&env, &feed_id, &submission);
        storage::set_feed_active_submission(&env, &feed_id, &submission);

        if let Some(latest) = aggregate_feed_latest(&env, &feed_id, current_time, staleness_window)
        {
            storage::set_feed_latest(&env, &feed_id, &latest);
        }

        // Update nonce
        storage::set_nonce(&env, &submitter, &feed_id, nonce);

        // Emit event
        events::emit_price_submitted(&env, &feed_id, price, &submitter, current_time);

        Ok(submission_id)
    }

    /// Get the latest price for a feed.
    pub fn get_latest_price(env: Env, feed_id: Symbol) -> Result<FeedLatest, OracleError> {
        storage::get_feed_latest(&env, &feed_id).ok_or(OracleError::FeedNotFound)
    }

    /// Get submission history for a feed (latest N submissions).
    pub fn get_price_history(
        env: Env,
        feed_id: Symbol,
        limit: u32,
    ) -> Result<soroban_sdk::Vec<PriceSubmission>, OracleError> {
        let history = storage::get_feed_history(&env, &feed_id, limit);
        if history.len() == 0 {
            return Err(OracleError::FeedNotFound);
        }
        Ok(history)
    }

    /// Set the staleness window (admin only).
    pub fn set_staleness_window(
        env: Env,
        caller: Address,
        seconds: u64,
    ) -> Result<(), OracleError> {
        shared::auth::require_admin(&env, &caller).map_err(|_| OracleError::Unauthorized)?;

        storage::set_staleness_window(&env, seconds);
        events::emit_staleness_window_set(&env, seconds, env.ledger().timestamp());
        Ok(())
    }

    /// Get the current staleness window.
    pub fn get_staleness_window(env: Env) -> u64 {
        storage::get_staleness_window(&env)
    }

    /// Check if a submitter is active.
    pub fn is_submitter_active(env: Env, submitter: Address) -> bool {
        storage::is_submitter_active(&env, &submitter)
    }

    /// Get the admin address.
    pub fn get_admin(env: Env) -> Address {
        shared::auth::get_admin(&env)
    }
}

fn aggregate_feed_latest(
    env: &Env,
    feed_id: &Symbol,
    current_time: u64,
    staleness_window: u64,
) -> Option<FeedLatest> {
    let active = storage::get_feed_active_submissions(env, feed_id);
    let mut prices = Vec::new(env);
    let mut decimals: Option<u32> = None;
    let mut newest_timestamp = 0_u64;

    for (_, submission) in active.iter() {
        if !storage::is_submitter_active(env, &submission.submitter) {
            continue;
        }
        let Some(expires_at) = submission.timestamp.checked_add(staleness_window) else {
            continue;
        };
        if submission.timestamp > current_time.saturating_add(MAX_FUTURE_SKEW_SECS)
            || current_time > expires_at
        {
            continue;
        }
        if let Some(expected_decimals) = decimals {
            if submission.decimals != expected_decimals {
                continue;
            }
        } else {
            decimals = Some(submission.decimals);
        }
        if submission.timestamp > newest_timestamp {
            newest_timestamp = submission.timestamp;
        }
        prices.push_back(submission.price);
    }

    if prices.len() < MIN_PRICE_QUORUM {
        return None;
    }

    sort_prices(&mut prices);
    let mid = prices.len() / 2;
    let price = if prices.len() % 2 == 1 {
        prices.get(mid).unwrap()
    } else {
        let low = prices.get(mid - 1).unwrap();
        let high = prices.get(mid).unwrap();
        low.checked_add(high)?.checked_div(2)?
    };

    Some(FeedLatest {
        feed_id: feed_id.clone(),
        price,
        decimals: decimals.unwrap_or(0),
        timestamp: newest_timestamp,
        submission_count: prices.len() as u64,
    })
}

fn sort_prices(prices: &mut Vec<i128>) {
    let len = prices.len();
    let mut i = 1;
    while i < len {
        let mut j = i;
        while j > 0 {
            let left = prices.get(j - 1).unwrap();
            let right = prices.get(j).unwrap();
            if left <= right {
                break;
            }
            prices.set(j - 1, right);
            prices.set(j, left);
            j -= 1;
        }
        i += 1;
    }
}
