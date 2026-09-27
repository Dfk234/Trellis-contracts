//! Fault-injection tests for the sandbox's fake external dependencies
//! (Issue #127).
//!
//! [`crate::sandbox`] already ships fault primitives on each fake adapter —
//! [`FakeOracleAdapter::fail_for`]/[`FakeOracleAdapter::mark_stale`],
//! [`FakeRpcAdapter::set_fail_next`], and [`FakeTokenAdapter`]'s ordinary
//! `Result` returns — but nothing exercised them end to end. This file is
//! that suite: it proves each fault surfaces as an actionable `Result`
//! rather than a panic, that a failed call leaves no partial state behind,
//! and that a retry after a transient fault behaves correctly.
//!
//! ## Running the fault suite
//!
//! ```bash
//! cargo test -p testing fault_injection
//! ```
//!
//! These tests are deterministic and fully in-process — no RPC, no real
//! token contract, no wall-clock time — so they are safe to run in CI on
//! every PR and never flake on external service availability.

#![cfg(test)]
extern crate std;

use crate::sandbox::{FakeOracleAdapter, FakeRpcAdapter, FakeTokenAdapter};
use shared::Error;
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, Env,
};

// ---------------------------------------------------------------------------
// Oracle: hard failure and staleness
// ---------------------------------------------------------------------------

/// A dependency that returns an error must surface it as `Err`, not panic —
/// and must not record a served quote for the failed call, so a caller who
/// retries after the outage clears sees an accurate `quotes_served` count.
#[test]
fn oracle_hard_failure_is_actionable_and_leaves_no_partial_state() {
    let env = Env::default();
    let asset = symbol_short!("XLM");
    let mut oracle = FakeOracleAdapter::new(&env, 1);
    oracle.set_price(&asset, 100).unwrap();

    oracle.fail_for(&asset);
    let result = oracle.quote(&env, &asset);
    assert_eq!(result, Err(Error::NotFound));
    assert_eq!(oracle.quotes_served(), 0, "a failed read must not count as served");

    // `fail_for` is a standing outage flag, not one-shot: it does not
    // self-clear like `FakeRpcAdapter::set_fail_next` does. A caller that
    // wants to simulate recovery must register a fresh adapter or re-set the
    // price; asserting that here pins the (deliberately) different fault
    // model from the RPC adapter below, so a future refactor that makes the
    // two adapters' fault behavior inconsistent is caught.
    let retry = oracle.quote(&env, &asset);
    assert_eq!(retry, Err(Error::NotFound));
}

/// Stale data must be flagged, not silently treated as fresh. A caller that
/// checks `PriceQuote::stale` before trusting the price is the "user-facing
/// error remains actionable" half of the acceptance criteria for a
/// dependency that degrades instead of failing outright.
#[test]
fn oracle_stale_price_is_flagged_not_silently_trusted() {
    let env = Env::default();
    let asset = symbol_short!("XLM");
    let mut oracle = FakeOracleAdapter::new(&env, 1);
    oracle.set_price(&asset, 100).unwrap();
    oracle.mark_stale(&asset);

    let quote = oracle.quote(&env, &asset).unwrap();
    assert!(quote.stale, "a marked-stale asset must report stale = true");
    assert_eq!(quote.price, 100, "the stale price is still returned for the caller to judge");
    assert_eq!(oracle.quotes_served(), 1, "a degraded-but-successful read still counts as served");
}

/// A price of zero (or negative) is rejected before it ever reaches storage
/// — the malformed-input analogue of a malformed upstream response.
#[test]
fn oracle_rejects_a_malformed_price_before_it_is_stored() {
    let env = Env::default();
    let asset = symbol_short!("XLM");
    let mut oracle = FakeOracleAdapter::new(&env, 1);

    assert_eq!(oracle.set_price(&asset, 0), Err(Error::InvalidAmount));
    assert_eq!(oracle.set_price(&asset, -5), Err(Error::InvalidAmount));
    // Nothing was ever stored, so a quote for the never-set asset is
    // NotFound rather than a bogus zero/negative price.
    assert_eq!(oracle.quote(&env, &asset), Err(Error::NotFound));
}

// ---------------------------------------------------------------------------
// Token: partial-write safety
// ---------------------------------------------------------------------------

/// An insufficient-balance transfer must reject before touching either
/// balance — the "no duplicate irreversible side effects" criterion applied
/// to a single failed write instead of a retried one.
#[test]
fn token_transfer_failure_leaves_both_balances_untouched() {
    let env = Env::default();
    env.mock_all_auths();
    let payer = Address::generate(&env);
    let payee = Address::generate(&env);
    let mut token = FakeTokenAdapter::new(&env, 1);
    token.mint(&payer, 100).unwrap();

    let result = token.transfer(&payer, &payee, 1_000);
    assert_eq!(result, Err(Error::InsufficientBalance));
    assert_eq!(token.balance_of(&payer), 100, "payer balance must be unchanged on a rejected transfer");
    assert_eq!(token.balance_of(&payee), 0, "payee balance must be unchanged on a rejected transfer");
    assert_eq!(token.total_supply(), 100, "total supply must be unaffected by a rejected transfer");
    assert_eq!(token.transfers(), 0, "a rejected transfer must not count as a completed one");
}

/// Retrying a failed transfer after topping up must succeed exactly once —
/// proof that the earlier failure didn't silently partially apply.
#[test]
fn token_transfer_retry_after_failure_succeeds_exactly_once() {
    let env = Env::default();
    env.mock_all_auths();
    let payer = Address::generate(&env);
    let payee = Address::generate(&env);
    let mut token = FakeTokenAdapter::new(&env, 1);
    token.mint(&payer, 50).unwrap();

    assert_eq!(token.transfer(&payer, &payee, 100), Err(Error::InsufficientBalance));
    token.mint(&payer, 100).unwrap();
    token.transfer(&payer, &payee, 100).unwrap();

    assert_eq!(token.balance_of(&payer), 50);
    assert_eq!(token.balance_of(&payee), 100);
    assert_eq!(token.transfers(), 1, "only the successful retry counts");
}

// ---------------------------------------------------------------------------
// RPC: submission timeout and retry
// ---------------------------------------------------------------------------

/// A submission timeout must fail without advancing the simulated ledger or
/// recording the operation — a caller that retries must not find the clock
/// already moved on, or the operation double-counted.
#[test]
fn rpc_submission_timeout_does_not_advance_the_ledger_or_record_the_op() {
    let env = Env::default();
    let op = symbol_short!("settle");
    let mut rpc = FakeRpcAdapter::new(&env, 1_000, 5_000, 3);
    rpc.set_fail_next(true);

    let result = rpc.submit(&op);
    assert_eq!(result, Err(Error::Expired));
    assert_eq!(rpc.ledger(), 1_000, "a failed submission must not advance the ledger");
    assert_eq!(rpc.timestamp(), 5_000);
    assert_eq!(rpc.submitted_count(), 0, "a failed submission must not be recorded as submitted");
}

/// After a timeout, retrying the same operation must succeed and advance the
/// clock exactly once — proving the fault was transient and non-duplicating.
#[test]
fn rpc_retry_after_timeout_succeeds_exactly_once() {
    let env = Env::default();
    let op = symbol_short!("settle");
    let mut rpc = FakeRpcAdapter::new(&env, 1_000, 5_000, 3);
    rpc.set_fail_next(true);

    assert_eq!(rpc.submit(&op), Err(Error::Expired));
    let landed_ledger = rpc.submit(&op).expect("retry after a transient timeout must succeed");

    assert_eq!(landed_ledger, 1_003, "the retry advances by exactly one latency window");
    assert_eq!(rpc.submitted_count(), 1, "only the successful retry is recorded, never both attempts");
}

/// `advance` itself is the deterministic clock every fault test above
/// depends on — pinned here so a change to its ledger/timestamp ratio is
/// visible instead of silently shifting every assertion above.
#[test]
fn rpc_advance_is_deterministic() {
    let env = Env::default();
    let mut rpc = FakeRpcAdapter::new(&env, 0, 0, 0);
    rpc.advance(10);
    assert_eq!(rpc.ledger(), 10);
    assert_eq!(rpc.timestamp(), 50);
}

// ---------------------------------------------------------------------------
// Cross-dependency: a partial workflow where only one leg fails
// ---------------------------------------------------------------------------

/// A workflow that reads a price, then fails to submit, must not have
/// consumed the oracle's serve count for nothing usable — the read
/// succeeded and is still valid for the retried submission. This is the
/// "partial write" scenario from the acceptance criteria: one dependency
/// degrades mid-workflow while the other's state must stay consistent.
#[test]
fn partial_workflow_failure_does_not_corrupt_the_dependency_that_succeeded() {
    let env = Env::default();
    let asset = symbol_short!("XLM");
    let op = symbol_short!("settle");

    let mut oracle = FakeOracleAdapter::new(&env, 1);
    oracle.set_price(&asset, 250).unwrap();
    let mut rpc = FakeRpcAdapter::new(&env, 1_000, 5_000, 2);

    let quote = oracle.quote(&env, &asset).expect("oracle leg succeeds");
    assert_eq!(quote.price, 250);
    assert_eq!(oracle.quotes_served(), 1);

    rpc.set_fail_next(true);
    assert_eq!(rpc.submit(&op), Err(Error::Expired));

    // The oracle's state is exactly what the successful read left behind —
    // the RPC leg's later failure did not roll it back or double-count it.
    assert_eq!(oracle.quotes_served(), 1);

    // The retried submission reuses the same already-valid quote and
    // succeeds on its own.
    assert!(rpc.submit(&op).is_ok());
    assert_eq!(rpc.submitted_count(), 1);
}
