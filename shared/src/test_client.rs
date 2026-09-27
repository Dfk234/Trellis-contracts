#![cfg(test)]

extern crate std;

use soroban_sdk::{symbol_short, testutils::Address as _, Address, BytesN, Env};
use crate::client::{
    client_schema_fingerprint, format_client_error, AidSummaryResponse, CreateAidRequest,
    CreateEscrowRequest, CreateListingRequest, CreateProposalRequest, EscrowSummaryResponse,
    ListingSummaryResponse, OperationStatus, ProposalSummaryResponse, RebalanceRequest,
    RebalanceSummaryResponse, CLIENT_SCHEMA_VERSION,
};
use crate::error_taxonomy::ErrorDomain;

#[test]
fn test_client_types_construction() {
    let env = Env::default();
    let addr1 = Address::generate(&env);
    let addr2 = Address::generate(&env);
    let token = Address::generate(&env);

    // Aid Request & Response
    let aid_req = CreateAidRequest {
        donor: addr1.clone(),
        recipient: addr2.clone(),
        amount: 1000,
        expiry_ledger: 100,
    };
    assert_eq!(aid_req.amount, 1000);

    let aid_res = AidSummaryResponse {
        aid_id: 1,
        donor: addr1.clone(),
        recipient: addr2.clone(),
        token: token.clone(),
        amount: 1000,
        expiry_ledger: 100,
        status: OperationStatus::Pending,
        schema_version: CLIENT_SCHEMA_VERSION,
    };
    assert_eq!(aid_res.status, OperationStatus::Pending);
    assert_eq!(aid_res.schema_version, 1);

    // Escrow Request & Response
    let escrow_req = CreateEscrowRequest {
        depositor: addr1.clone(),
        beneficiary: addr2.clone(),
        token: token.clone(),
        amount: 500,
        expiry_ledger: 200,
    };
    assert_eq!(escrow_req.amount, 500);

    let escrow_res = EscrowSummaryResponse {
        escrow_id: 42,
        depositor: addr1.clone(),
        beneficiary: addr2.clone(),
        token: token.clone(),
        amount: 500,
        fee_amount: 5,
        expiry_ledger: 200,
        status: OperationStatus::Active,
    };
    assert_eq!(escrow_res.status, OperationStatus::Active);

    // Proposal Request & Response
    let prop_req = CreateProposalRequest {
        title: symbol_short!("upgrade"),
        target: addr1.clone(),
        action_type: 1,
        parameter_key: 0,
        parameter_value: 100,
    };
    assert_eq!(prop_req.action_type, 1);

    let prop_res = ProposalSummaryResponse {
        proposal_id: 10,
        proposer: addr1.clone(),
        status: OperationStatus::Completed,
        approval_count: 3,
        threshold: 2,
        created_at: 1000,
        expires_at: 2000,
    };
    assert_eq!(prop_res.approval_count, 3);

    // Listing Request & Response
    let list_req = CreateListingRequest {
        seller: addr1.clone(),
        collection: addr2.clone(),
        token_id: 7,
        price: 250,
        currency: token.clone(),
    };
    assert_eq!(list_req.token_id, 7);

    let list_res = ListingSummaryResponse {
        listing_id: 99,
        seller: addr1.clone(),
        collection: addr2.clone(),
        token_id: 7,
        price: 250,
        currency: token.clone(),
        status: OperationStatus::Active,
    };
    assert_eq!(list_res.price, 250);

    // Rebalance Request & Response
    let rebal_req = RebalanceRequest {
        asset_in: symbol_short!("XLM"),
        asset_out: symbol_out(),
        amount: 10_000,
        max_slippage_bps: 50,
    };
    assert_eq!(rebal_req.max_slippage_bps, 50);

    let rebal_res = RebalanceSummaryResponse {
        status: OperationStatus::Completed,
        executed_amount: 10_000,
        fee_paid: 10,
        actual_slippage_bps: 12,
    };
    assert_eq!(rebal_res.actual_slippage_bps, 12);
}

fn symbol_out() -> soroban_sdk::Symbol {
    symbol_short!("USDC")
}

#[test]
fn test_client_error_formatting() {
    let env = Env::default();
    let correlation_id = BytesN::from_array(&env, &[7u8; 32]);

    // Unauthorized error code (1) in Shared domain -> non-retryable
    let formatted = format_client_error(&env, ErrorDomain::Shared, 1, correlation_id.clone());
    assert_eq!(formatted.domain, symbol_short!("shared"));
    assert_eq!(formatted.code, 1);
    assert_eq!(formatted.category, symbol_short!("auth"));
    assert_eq!(formatted.retryable, false);
    assert_eq!(formatted.correlation_id, correlation_id);
}

#[test]
fn test_client_schema_fingerprint() {
    let env = Env::default();
    let fp1 = client_schema_fingerprint(&env);
    let fp2 = client_schema_fingerprint(&env);
    assert_eq!(fp1, fp2);
    assert_ne!(fp1, BytesN::from_array(&env, &[0u8; 32]));
}
