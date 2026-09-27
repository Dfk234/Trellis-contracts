#![cfg(test)]

extern crate std;

use soroban_sdk::{symbol_short, Env, String};

use crate::disclosure::{DetailField, DetailSeverity, TransactionDetail, TransactionDetailBuilder};

fn labels(fields: &soroban_sdk::Vec<DetailField>) -> std::vec::Vec<soroban_sdk::Symbol> {
    fields.iter().map(|f| f.label).collect()
}

#[test]
fn info_and_warning_fields_land_where_requested() {
    let env = Env::default();
    let detail = TransactionDetailBuilder::new(&env)
        .push(
            symbol_short!("amount"),
            String::from_str(&env, "1000"),
            DetailSeverity::Info,
            false,
        )
        .push(
            symbol_short!("route"),
            String::from_str(&env, "escrow"),
            DetailSeverity::Warning,
            true,
        )
        .finish();

    assert_eq!(detail.summary.len(), 1);
    assert_eq!(detail.advanced.len(), 1);
    assert_eq!(labels(&detail.summary), std::vec![symbol_short!("amount")]);
    assert_eq!(labels(&detail.advanced), std::vec![symbol_short!("route")]);
}

/// The core invariant: a `Critical` field requested as advanced is placed in
/// `summary` anyway. This is the "critical warnings are never hidden only in
/// advanced sections" acceptance criterion, and it is enforced structurally —
/// this test would fail if `push` ever stopped overriding the request.
#[test]
fn critical_field_is_promoted_to_summary_even_when_requested_as_advanced() {
    let env = Env::default();
    let detail = TransactionDetailBuilder::new(&env)
        .push(
            symbol_short!("expiry"),
            String::from_str(&env, "expires in 2 ledgers"),
            DetailSeverity::Critical,
            true, // requested advanced — must be overridden
        )
        .finish();

    assert_eq!(detail.summary.len(), 1, "the critical field must be in summary");
    assert_eq!(detail.advanced.len(), 0, "and never in advanced");
    let field = detail.summary.get(0).unwrap();
    assert_eq!(field.severity, DetailSeverity::Critical);
    // The original request is preserved for display ("always shown despite
    // being tagged advanced"), even though it was not honored.
    assert!(field.requested_advanced);
}

#[test]
fn expanded_view_contains_every_field_collapsed_view_does_not_lose_any() {
    let env = Env::default();
    let detail = TransactionDetailBuilder::new(&env)
        .push(symbol_short!("a"), String::from_str(&env, "1"), DetailSeverity::Info, false)
        .push(symbol_short!("b"), String::from_str(&env, "2"), DetailSeverity::Info, true)
        .push(symbol_short!("c"), String::from_str(&env, "3"), DetailSeverity::Critical, true)
        .finish();

    // Collapsed (summary only): "a" (not advanced) + "c" (critical override).
    assert_eq!(detail.summary.len(), 2);
    // Expanded: all three, nothing lost, nothing duplicated.
    let expanded = detail.expanded();
    assert_eq!(expanded.len(), 3);
    assert_eq!(
        labels(&expanded),
        std::vec![symbol_short!("a"), symbol_short!("c"), symbol_short!("b")]
    );
}

#[test]
fn empty_detail_has_an_empty_expanded_view() {
    let env = Env::default();
    let detail = TransactionDetailBuilder::new(&env).finish();
    assert_eq!(detail.summary.len(), 0);
    assert_eq!(detail.advanced.len(), 0);
    assert_eq!(detail.expanded().len(), 0);
}

#[test]
fn critical_fields_are_never_advanced_only_holds_for_builder_output() {
    let env = Env::default();
    let detail = TransactionDetailBuilder::new(&env)
        .push(symbol_short!("x"), String::from_str(&env, "1"), DetailSeverity::Critical, true)
        .push(symbol_short!("y"), String::from_str(&env, "2"), DetailSeverity::Warning, true)
        .finish();
    assert!(detail.critical_fields_are_never_advanced_only());
}

/// The invariant helper must actually be able to say `false` — otherwise it
/// would be checking nothing. A `TransactionDetail` assembled directly
/// (bypassing the builder, e.g. from a deserialized/legacy payload) can
/// violate it, which is exactly the case the helper exists to catch.
#[test]
fn critical_fields_are_never_advanced_only_detects_a_hand_built_violation() {
    let env = Env::default();
    let mut advanced = soroban_sdk::Vec::new(&env);
    advanced.push_back(crate::disclosure::DetailField {
        label: symbol_short!("z"),
        value: String::from_str(&env, "hidden risk"),
        severity: DetailSeverity::Critical,
        requested_advanced: true,
    });
    let detail = TransactionDetail {
        summary: soroban_sdk::Vec::new(&env),
        advanced,
    };
    assert!(!detail.critical_fields_are_never_advanced_only());
}
