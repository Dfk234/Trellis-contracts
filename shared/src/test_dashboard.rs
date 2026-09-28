#![cfg(test)]

use soroban_sdk::{testutils::Env as _, Env, Vec};
use crate::dashboard::{generate_dashboard};
use crate::reconciliation::SourceRecord;
use crate::health::{set_dependency_health, DependencyStatus};
use soroban_sdk::symbol_short;

#[test]
fn test_dashboard_aggregation() {
    let env = Env::default();
    
    // Simulate some incidents
    set_dependency_health(&env, &symbol_short!("indexer"), DependencyStatus::Down);
    set_dependency_health(&env, &symbol_short!("rpc"), DependencyStatus::Healthy);

    let ledger_records = Vec::new(&env);
    let db_records = Vec::new(&env);
    let user_records = Vec::new(&env);
    
    let report = generate_dashboard(&env, ledger_records, db_records, user_records, 100);
    
    // We expect 3 categories
    assert_eq!(report.categories.len(), 3);
    
    let incidents = report.categories.get(2).unwrap();
    assert_eq!(incidents.count, 1);
    assert_eq!(incidents.actionable, true);

    let drifts = report.categories.get(1).unwrap();
    assert_eq!(drifts.count, 0);
    assert_eq!(drifts.actionable, false);

    let jobs = report.categories.get(0).unwrap();
    assert_eq!(jobs.count, 0);
    assert_eq!(jobs.actionable, false);
}
