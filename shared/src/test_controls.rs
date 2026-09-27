use crate::{
    get_dependency_health, list_dependency_health, open_recovery, record_abuse, set_abuse_policy,
    set_dependency_health, update_recovery_status, validate_amount, validate_distinct_parties,
    validate_future_expiry, AbusePolicy, AmountRule, DependencyStatus, Error, ExpiryRule,
    RecoveryStatus,
};
use soroban_sdk::{contract, contractimpl, symbol_short, testutils::Address as _, Address, Env};

#[contract]
struct DummyContract;

#[contractimpl]
impl DummyContract {}

fn with_contract<F: FnOnce(&Env, Address)>(f: F) {
    let env = Env::default();
    let contract = env.register_contract(None, DummyContract);
    env.as_contract(&contract, || f(&env, contract.clone()));
}

#[test]
fn abuse_score_blocks_when_policy_is_exceeded() {
    with_contract(|env, _| {
        let actor = Address::generate(env);
        let resource = symbol_short!("reb");
        set_abuse_policy(
            env,
            &resource,
            &AbusePolicy {
                max_score: 10,
                decay_per_window: 1,
                window_ledgers: 100,
            },
        )
        .unwrap();
        assert!(record_abuse(env, &actor, &resource, 5).is_ok());
        assert_eq!(
            record_abuse(env, &actor, &resource, 6),
            Err(Error::QuotaExceeded)
        );
    });
}

#[test]
fn recovery_records_are_user_queryable() {
    with_contract(|env, _| {
        let actor = Address::generate(env);
        let record = open_recovery(env, &actor, &symbol_short!("claim"), Error::NotFound as u32);
        assert_eq!(record.status, RecoveryStatus::Pending);
        let updated = update_recovery_status(env, record.id, RecoveryStatus::Resolved).unwrap();
        assert_eq!(updated.status, RecoveryStatus::Resolved);
        assert_eq!(crate::list_recoveries(env, &actor, 10).len(), 1);
    });
}

#[test]
fn semantic_validation_catches_edge_cases() {
    with_contract(|env, _| {
        let a = Address::generate(env);
        assert_eq!(
            validate_distinct_parties(&a, &a),
            Err(Error::InvalidArgument)
        );
        assert_eq!(
            validate_amount(
                0,
                &AmountRule {
                    min: 1,
                    max: 100,
                    allow_zero: false,
                },
            ),
            Err(Error::InvalidAmount),
        );
        assert!(validate_future_expiry(
            env,
            env.ledger().sequence() + 5,
            &ExpiryRule {
                min_delay_ledgers: 1,
                max_delay_ledgers: 10,
            },
        )
        .is_ok());
    });
}

#[test]
fn dependency_health_records_latest_status() {
    with_contract(|env, _| {
        let name = symbol_short!("rpc");
        set_dependency_health(env, &name, DependencyStatus::Degraded);
        let health = get_dependency_health(env, &name).unwrap();
        assert_eq!(health.status, DependencyStatus::Degraded);
        assert_eq!(health.failure_count, 1);
        set_dependency_health(env, &name, DependencyStatus::Healthy);
        assert_eq!(list_dependency_health(env).len(), 1);
        assert_eq!(get_dependency_health(env, &name).unwrap().failure_count, 0);
    });
}
