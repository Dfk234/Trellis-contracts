#![cfg(test)]

extern crate std;

use super::*;
use shared::Error as SharedError;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Env,
};
#[allow(dead_code)]
fn setup_token<'a>(
    env: &'a Env,
    admin: &Address,
) -> (Address, token::Client<'a>, token::StellarAssetClient<'a>) {
    let contract_address = env.register_stellar_asset_contract(admin.clone());
    let client = token::Client::new(env, &contract_address);
    let asset_client = token::StellarAssetClient::new(env, &contract_address);
    (contract_address, client, asset_client)
}

const MINT_AMOUNT: i128 = 1_000_000;

struct Fixture {
    env: Env,
    admin: Address,
    donor: Address,
    recipient: Address,
    token_addr: Address,
    contract_id: Address,
}

fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let donor = Address::generate(&env);
    let recipient = Address::generate(&env);

    let token_addr = env.register_stellar_asset_contract(admin.clone());
    let asset_client = token::StellarAssetClient::new(&env, &token_addr);
    asset_client.mint(&donor, &MINT_AMOUNT);

    let contract_id = env.register_contract(None, AidContract);
    let client = AidContractClient::new(&env, &contract_id);
    let treasury = Address::generate(&env);
    client.initialize(&admin, &treasury, &token_addr, &3600);

    Fixture {
        env,
        admin,
        donor,
        recipient,
        token_addr,
        contract_id,
    }
}

/// Create `count` aids of 100 units each from `donor` to `recipient`,
/// returning the allocated IDs in creation order.
fn create_aids(
    env: &Env,
    client: &AidContractClient,
    donor: &Address,
    recipient: &Address,
    count: u32,
) -> std::vec::Vec<u64> {
    let expiry = env.ledger().sequence() + 10_000;
    let mut ids = std::vec::Vec::with_capacity(count as usize);
    for _ in 0..count {
        ids.push(client.create_aid(donor, recipient, &100, &expiry));
    }
    ids
}

fn advance_ledger(env: &Env, delta: u32) {
    env.ledger().with_mut(|l| {
        l.sequence_number += delta;
    });
}

// ---------------------------------------------------------------------------
// Claim lifecycle
// ---------------------------------------------------------------------------

#[test]
fn claim_transfers_escrow_and_settles() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);
    let token_client = token::Client::new(&fx.env, &fx.token_addr);

    let expiry = fx.env.ledger().sequence() + 100;
    let aid_id = client.create_aid(&fx.donor, &fx.recipient, &500, &expiry);
    assert_eq!(token_client.balance(&fx.contract_id), 500);

    client.claim_aid(&aid_id, &fx.recipient);

    assert_eq!(token_client.balance(&fx.recipient), 500);
    assert_eq!(token_client.balance(&fx.contract_id), 0);

    let record = client.get_aid(&aid_id).unwrap();
    assert_eq!(record.status, AidStatus::Settled);
}

#[test]
fn second_claim_returns_already_claimed() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    let expiry = fx.env.ledger().sequence() + 100;
    let aid_id = client.create_aid(&fx.donor, &fx.recipient, &500, &expiry);
    client.claim_aid(&aid_id, &fx.recipient);

    let result = client.try_claim_aid(&aid_id, &fx.recipient);
    assert_eq!(result, Err(Ok(AidError::AlreadyClaimed)));
}

#[test]
fn claim_after_expiry_is_rejected() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    let expiry = fx.env.ledger().sequence() + 100;
    let aid_id = client.create_aid(&fx.donor, &fx.recipient, &500, &expiry);

    advance_ledger(&fx.env, 101);

    let result = client.try_claim_aid(&aid_id, &fx.recipient);
    assert_eq!(result, Err(Ok(AidError::Expired)));
}

#[test]
fn claim_by_wrong_address_is_unauthorized() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);
    let stranger = Address::generate(&fx.env);

    let expiry = fx.env.ledger().sequence() + 100;
    let aid_id = client.create_aid(&fx.donor, &fx.recipient, &500, &expiry);

    let result = client.try_claim_aid(&aid_id, &stranger);
    assert_eq!(result, Err(Ok(AidError::Unauthorized)));
}

#[test]
fn claim_while_paused_is_rejected() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    let expiry = fx.env.ledger().sequence() + 100;
    let aid_id = client.create_aid(&fx.donor, &fx.recipient, &500, &expiry);
    client.set_paused(&fx.admin, &true);

    let result = client.try_claim_aid(&aid_id, &fx.recipient);
    assert_eq!(result, Err(Ok(AidError::Paused)));
}

#[test]
fn pauser_permission_can_be_granted_and_revoked() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);
    let pauser = Address::generate(&fx.env);

    assert_eq!(
        client.try_set_pauser(&fx.donor, &pauser, &true),
        Err(Ok(SharedError::Unauthorized))
    );
    client.set_pauser(&fx.admin, &pauser, &true);
    client.set_paused(&pauser, &true);
    let paused = fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .instance()
            .get::<_, bool>(&Symbol::new(&fx.env, "paused"))
            .unwrap_or(false)
    });
    assert!(paused);

    client.set_paused(&fx.admin, &false);
    client.set_pauser(&fx.admin, &pauser, &false);
    assert!(client.try_set_paused(&pauser, &true).is_err());
}

#[test]
fn create_aid_rejects_non_positive_amount_and_past_expiry() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    let expiry = fx.env.ledger().sequence() + 100;
    assert_eq!(
        client.try_create_aid(&fx.donor, &fx.recipient, &0, &expiry),
        Err(Ok(soroban_sdk::Error::from_contract_error(
            SharedError::InvalidAmount as u32
        )))
    );

    let past = fx.env.ledger().sequence();
    assert_eq!(
        client.try_create_aid(&fx.donor, &fx.recipient, &100, &past),
        Err(Ok(soroban_sdk::Error::from_contract_error(
            SharedError::InvalidArgument as u32
        )))
    );
}

// ---------------------------------------------------------------------------
// Refunds
// ---------------------------------------------------------------------------

#[test]
fn refund_aid_after_expiry_returns_funds_to_donor() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);
    let token_client = token::Client::new(&fx.env, &fx.token_addr);

    let expiry = fx.env.ledger().sequence() + 100;
    let aid_id = client.create_aid(&fx.donor, &fx.recipient, &500, &expiry);
    advance_ledger(&fx.env, 101);

    client.refund_aid(&aid_id, &fx.donor);

    assert_eq!(token_client.balance(&fx.donor), MINT_AMOUNT);
    assert_eq!(token_client.balance(&fx.contract_id), 0);
    let record = client.get_aid(&aid_id).unwrap();
    assert_eq!(record.status, AidStatus::Refunded);
}

#[test]
fn refund_aid_before_expiry_is_rejected() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    let expiry = fx.env.ledger().sequence() + 100;
    let aid_id = client.create_aid(&fx.donor, &fx.recipient, &500, &expiry);

    let result = client.try_refund_aid(&aid_id, &fx.donor);
    assert_eq!(result, Err(Ok(AidError::NotExpiredYet)));
}

#[test]
fn refund_claimed_aid_is_rejected() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    let expiry = fx.env.ledger().sequence() + 100;
    let aid_id = client.create_aid(&fx.donor, &fx.recipient, &500, &expiry);
    client.claim_aid(&aid_id, &fx.recipient);
    advance_ledger(&fx.env, 101);

    let result = client.try_refund_aid(&aid_id, &fx.donor);
    assert_eq!(result, Err(Ok(AidError::AlreadyClaimed)));
}

#[test]
fn refund_refunded_aid_is_rejected() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    let expiry = fx.env.ledger().sequence() + 100;
    let aid_id = client.create_aid(&fx.donor, &fx.recipient, &500, &expiry);
    advance_ledger(&fx.env, 101);
    client.refund_aid(&aid_id, &fx.donor);

    let result = client.try_refund_aid(&aid_id, &fx.donor);
    assert_eq!(result, Err(Ok(AidError::AlreadyRefunded)));
}

#[test]
fn refund_by_admin_is_successful() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);
    let token_client = token::Client::new(&fx.env, &fx.token_addr);

    let expiry = fx.env.ledger().sequence() + 100;
    let aid_id = client.create_aid(&fx.donor, &fx.recipient, &500, &expiry);
    advance_ledger(&fx.env, 101);

    client.refund_aid(&aid_id, &fx.admin);

    assert_eq!(token_client.balance(&fx.donor), MINT_AMOUNT);
    let record = client.get_aid(&aid_id).unwrap();
    assert_eq!(record.status, AidStatus::Refunded);
}

// ---------------------------------------------------------------------------
// Single-record queries
// ---------------------------------------------------------------------------

#[test]
fn get_aid_unknown_id_returns_none() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    assert_eq!(client.get_aid(&9_999), None);
}

#[test]
fn get_aid_returns_full_record() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    let expiry = fx.env.ledger().sequence() + 100;
    let aid_id = client.create_aid(&fx.donor, &fx.recipient, &250, &expiry);

    let record = client.get_aid(&aid_id).expect("record should exist");
    assert_eq!(record.id, aid_id);
    assert_eq!(record.donor, fx.donor);
    assert_eq!(record.recipient, fx.recipient);
    assert_eq!(record.token, fx.token_addr);
    assert_eq!(record.amount, 250);
    assert_eq!(record.expiry_ledger, expiry);
    assert_eq!(record.status, AidStatus::Pending);
}

#[test]
fn aid_ids_are_unique_and_monotonic() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    let ids = create_aids(&fx.env, &client, &fx.donor, &fx.recipient, 5);
    let sorted = ids.clone();
    assert_eq!(ids, sorted);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(*id, i as u64);
    }
}

// ---------------------------------------------------------------------------
// Permission-aware discovery index
// ---------------------------------------------------------------------------

#[test]
fn search_filters_restricted_records_and_honors_permission_revocation() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);
    let outsider = Address::generate(&fx.env);
    let delegate = Address::generate(&fx.env);
    let aid_id = client.create_aid(
        &fx.donor,
        &fx.recipient,
        &100,
        &(fx.env.ledger().sequence() + 100),
    );

    assert_eq!(client.search_aids(&outsider, &0, &10).records.len(), 0);
    assert_eq!(client.search_aids(&fx.recipient, &0, &10).records.len(), 1);

    client.grant_search_access(&fx.donor, &aid_id, &delegate);
    assert_eq!(client.search_aids(&delegate, &0, &10).records.len(), 1);
    client.revoke_search_access(&fx.donor, &aid_id, &delegate);
    assert_eq!(client.search_aids(&delegate, &0, &10).records.len(), 0);
}

#[test]
fn visibility_change_and_deletion_remove_discovery_entries() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);
    let aid_id = client.create_aid(
        &fx.donor,
        &fx.recipient,
        &100,
        &(fx.env.ledger().sequence() + 100),
    );

    client.set_aid_search_visibility(&fx.admin, &aid_id, &false);
    assert_eq!(client.search_aids(&fx.donor, &0, &10).records.len(), 0);
    client.set_aid_search_visibility(&fx.admin, &aid_id, &true);
    assert_eq!(client.search_aids(&fx.donor, &0, &10).records.len(), 1);

    client.claim_aid(&aid_id, &fx.recipient);
    assert_eq!(client.search_aids(&fx.donor, &0, &10).records.len(), 0);
    client.delete_aid(&fx.admin, &aid_id);
    assert_eq!(client.get_aid(&aid_id), None);
}

#[test]
fn repair_search_index_restores_missing_entries_and_removes_stale_ones() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);
    let aid_id = client.create_aid(
        &fx.donor,
        &fx.recipient,
        &100,
        &(fx.env.ledger().sequence() + 100),
    );

    // Simulate a partial indexer write and a dangling entry from evicted data.
    fx.env.as_contract(&fx.contract_id, || {
        let mut corrupt = Vec::new(&fx.env);
        corrupt.push_back(99_999);
        storage::set_search_index(&fx.env, &corrupt);
    });

    let report = client.repair_search_index(&fx.admin);
    assert_eq!(report.indexed, 1);
    assert_eq!(report.added, 1);
    assert_eq!(report.removed, 1);
    let results = client.search_aids(&fx.donor, &0, &10);
    assert_eq!(results.records.len(), 1);
    assert_eq!(results.records.get(0).unwrap().id, aid_id);
}

#[test]
fn test_stable_cursor_pagination_on_aid_contract() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    // Create 5 aids
    let mut ids = std::vec::Vec::new();
    let expiry = fx.env.ledger().sequence() + 1000;
    for _ in 0..5 {
        ids.push(client.create_aid(&fx.donor, &fx.recipient, &100, &expiry));
    }

    // Page 1: limit 2
    let page1 = client.list_aids_by_donor_cursor(&fx.donor, &None, &2);
    assert_eq!(page1.records.len(), 2);
    assert_eq!(page1.records.get(0).unwrap().id, ids[0]);
    assert_eq!(page1.records.get(1).unwrap().id, ids[1]);
    assert_eq!(page1.next_cursor, Some(ids[1]));
    assert!(page1.has_more);

    // Page 2: limit 2, start_after_id = ids[1]
    let page2 = client.list_aids_by_donor_cursor(&fx.donor, &page1.next_cursor, &2);
    assert_eq!(page2.records.len(), 2);
    assert_eq!(page2.records.get(0).unwrap().id, ids[2]);
    assert_eq!(page2.records.get(1).unwrap().id, ids[3]);
    assert_eq!(page2.next_cursor, Some(ids[3]));
    assert!(page2.has_more);

    // Page 3: limit 2, start_after_id = ids[3]
    let page3 = client.list_aids_by_donor_cursor(&fx.donor, &page2.next_cursor, &2);
    assert_eq!(page3.records.len(), 1);
    assert_eq!(page3.records.get(0).unwrap().id, ids[4]);
    assert_eq!(page3.next_cursor, None);
    assert!(!page3.has_more);

    // Test search_aids_cursor
    let search_page = client.search_aids_cursor(&fx.donor, &None, &3);
    assert_eq!(search_page.records.len(), 3);
    assert_eq!(search_page.next_cursor, Some(ids[2]));
}

#[test]
fn test_aid_contract_import_dry_run_and_commit() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    let ext_1 = Bytes::from_slice(&fx.env, b"AID-EXT-01");
    let ext_2 = Bytes::from_slice(&fx.env, b"AID-EXT-02");

    let mut items = Vec::new(&fx.env);
    items.push_back(shared::import::ImportItem {
        row_id: 0,
        external_id: Some(ext_1.clone()),
        recipient: fx.recipient.clone(),
        amount: 500,
        expiry: fx.env.ledger().timestamp() + 3600,
        metadata_hash: None,
    });
    items.push_back(shared::import::ImportItem {
        row_id: 1,
        external_id: Some(ext_2.clone()),
        recipient: fx.recipient.clone(),
        amount: 800,
        expiry: fx.env.ledger().timestamp() + 7200,
        metadata_hash: None,
    });

    let config = shared::import::ImportConfig {
        dry_run: false,
        mode: shared::import::ImportMode::AllOrNothing,
        duplicate_policy: shared::import::DuplicatePolicy::SkipExisting,
        max_rows: 50,
    };

    // 1. Dry run via contract entrypoint
    let dry_report = client.import_aids_dry_run(&items, &config);
    assert!(dry_report.is_dry_run);
    assert_eq!(dry_report.total_rows, 2);
    assert_eq!(dry_report.create_count, 2);
    assert_eq!(dry_report.error_count, 0);

    // Acceptance criteria check: Dry run performs no persistent writes
    assert_eq!(client.get_imported_aid(&ext_1), None);
    assert_eq!(client.get_imported_aid(&ext_2), None);

    // 2. Commit execution via contract entrypoint
    let commit_report = client.import_aids(&fx.donor, &items, &config);
    assert!(!commit_report.is_dry_run);
    assert_eq!(commit_report.create_count, 2);
    assert_eq!(commit_report.error_count, 0);

    // Verify stored records exist
    let rec1 = client.get_imported_aid(&ext_1).expect("rec1 should be persisted");
    assert_eq!(rec1.amount, 500);
    assert_eq!(rec1.recipient, fx.recipient);

    let rec2 = client.get_imported_aid(&ext_2).expect("rec2 should be persisted");
    assert_eq!(rec2.amount, 800);

    // 3. Acceptance criteria check: Repeated imports are idempotent where external IDs are present
    let rerun_report = client.import_aids(&fx.donor, &items, &config);
    assert_eq!(rerun_report.create_count, 0);
    assert_eq!(rerun_report.skip_count, 2);
    assert_eq!(rerun_report.update_count, 0);
    assert_eq!(rerun_report.error_count, 0);
}

#[test]
fn test_aid_contract_import_partial_failure_handling() {
    let fx = setup();
    let client = AidContractClient::new(&fx.env, &fx.contract_id);

    let ext_good = Bytes::from_slice(&fx.env, b"AID-GOOD");
    let ext_bad = Bytes::from_slice(&fx.env, b"AID-BAD");

    let mut items = Vec::new(&fx.env);
    items.push_back(shared::import::ImportItem {
        row_id: 0,
        external_id: Some(ext_good.clone()),
        recipient: fx.recipient.clone(),
        amount: 300,
        expiry: fx.env.ledger().timestamp() + 5000,
        metadata_hash: None,
    });
    items.push_back(shared::import::ImportItem {
        row_id: 1,
        external_id: Some(ext_bad.clone()),
        recipient: fx.recipient.clone(),
        amount: 0, // Invalid amount: triggers row failure
        expiry: fx.env.ledger().timestamp() + 5000,
        metadata_hash: None,
    });

    let config = shared::import::ImportConfig {
        dry_run: false,
        mode: shared::import::ImportMode::BestEffort,
        duplicate_policy: shared::import::DuplicatePolicy::SkipExisting,
        max_rows: 50,
    };

    let report = client.import_aids(&fx.donor, &items, &config);
    assert_eq!(report.total_rows, 2);
    assert_eq!(report.create_count, 1);
    assert_eq!(report.error_count, 1);
    assert_eq!(report.errors.len(), 1);

    // Check error details and rollback guidance
    let err = report.errors.get(0).unwrap();
    assert_eq!(err.row_id, 1);
    assert_eq!(err.reason, symbol_short!("zero_amt"));
    assert_eq!(report.rollback_guidance.strategy, shared::import::RollbackStrategy::ForwardFix);
    assert_eq!(report.rollback_guidance.action, symbol_short!("part_fix"));

    // Valid row is committed, invalid row is not
    assert!(client.get_imported_aid(&ext_good).is_some());
    assert!(client.get_imported_aid(&ext_bad).is_none());
}

