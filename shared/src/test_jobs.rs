//! Validation tests for the background worker framework (Issue #35).
//!
//! These tests are the issue's validation artifact: they cover enqueueing,
//! retry with backoff, retry exhaustion into the dead-letter queue, idempotent
//! reprocessing, failure context preservation, and admin management.

extern crate std;

use std::cell::Cell;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{contract, contractimpl, symbol_short, token, Address, BytesN, Env};

use crate::auth::set_admin;
use crate::errors::Error;
use crate::jobs::{
    configure_worker, dead_letter_job_ids, dedupe_key_u64, discard_dead_letter, enqueue_escrow_refund,
    enqueue_job, get_job, get_receipt, job_stats, next_due_ledger, pause_worker, pending_job_ids,
    reprocess_job, requeue_dead_letter, resume_worker, run_due_job, worker_config,
    EnqueueOutcome, Job, JobError, JobHandler, JobKind, JobPayload, JobStatus, RetryPolicy,
    ESCROW_REFUND_TAG,
    RunOutcome, WorkerConfig,
};
use crate::payments::{
    create_escrow_with_refund_job, get_escrow, release_escrow, schedule_escrow_refund,
    EscrowRefundHandler, EscrowState,
};

#[contract]
pub struct JobFixture;

#[contractimpl]
impl JobFixture {
    pub fn noop(_env: Env) {}

    /// Test wrapper mirroring what a payments contract does: tie the
    /// depositor's authorization to the root invocation, then create the
    /// escrow and schedule its refund job.
    pub fn create_escrow_schedule_refund(
        env: Env,
        token: Address,
        depositor: Address,
        beneficiary: Address,
        amount: i128,
        expiry_ledger: u32,
    ) -> Result<(u64, EnqueueOutcome), Error> {
        depositor.require_auth();
        crate::payments::create_escrow_with_refund_job(
            &env,
            &token,
            &depositor,
            &beneficiary,
            amount,
            expiry_ledger,
        )
    }
}

/// Handler whose outcome is scripted per call: the first `failures` calls fail
/// with `Error::Expired`, later calls succeed.
struct ScriptedHandler {
    calls: Cell<u32>,
    failures: u32,
}

impl ScriptedHandler {
    fn new(failures: u32) -> Self {
        ScriptedHandler {
            calls: Cell::new(0),
            failures,
        }
    }

    fn calls(&self) -> u32 {
        self.calls.get()
    }
}

impl JobHandler for ScriptedHandler {
    fn handle(&self, _env: &Env, _job: &Job) -> Result<(), Error> {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        if call <= self.failures {
            Err(Error::Expired)
        } else {
            Ok(())
        }
    }
}

fn key(env: &Env, tag: u32, value: u64) -> BytesN<32> {
    dedupe_key_u64(env, tag, value)
}

/// Registers a fixture contract and sets an admin. Returns (contract, admin).
fn setup(env: &Env) -> (Address, Address) {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let contract_id = env.register_contract(None, JobFixture);
    env.as_contract(&contract_id, || {
        set_admin(env, &admin);
    });
    (contract_id, admin)
}

#[test]
fn enqueue_is_idempotent_for_the_same_dedupe_key() {
    let env = Env::default();
    let (contract_id, _admin) = setup(&env);

    env.as_contract(&contract_id, || {
        let first = enqueue_job(&env, JobPayload::None, key(&env, 1, 7), 0, None).unwrap();
        let second = enqueue_job(&env, JobPayload::None, key(&env, 1, 7), 0, None).unwrap();

        assert_eq!(first, EnqueueOutcome::Created(1));
        assert_eq!(second, EnqueueOutcome::AlreadyPending(1));
        assert_eq!(pending_job_ids(&env).len(), 1);
        assert_eq!(job_stats(&env).counters.enqueued_total, 1);
    });
}

#[test]
fn successful_run_writes_a_receipt_and_replaying_is_idempotent() {
    let env = Env::default();
    let (contract_id, _admin) = setup(&env);
    let handler = ScriptedHandler::new(0);

    env.as_contract(&contract_id, || {
        let worker = Address::generate(&env);
        let EnqueueOutcome::Created(id) =
            enqueue_job(&env, JobPayload::None, key(&env, 3, 1), 0, None).unwrap()
        else {
            panic!("expected a new job")
        };

        let RunOutcome::Succeeded(job) = run_due_job(&env, &worker, 5, &handler).unwrap() else {
            panic!("expected the job to succeed")
        };
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.completed_ledger, 5);
        assert_eq!(job.attempts, 1);
        assert_eq!(handler.calls(), 1);
        assert_eq!(pending_job_ids(&env).len(), 0);
        assert_eq!(next_due_ledger(&env), None);

        let receipt = get_receipt(&env, &key(&env, 3, 1)).unwrap();
        assert_eq!(receipt.job_id, id);
        assert_eq!(receipt.attempts, 1);
        assert_eq!(receipt.completed_ledger, 5);
        assert_eq!(receipt.correlation_id, 0);

        // Replaying the same job returns the receipt without re-running work.
        assert_eq!(
            reprocess_job(&env, &worker, id, 9, &handler).unwrap(),
            RunOutcome::AlreadyCompleted(receipt)
        );
        assert_eq!(handler.calls(), 1);

        // And the same logical job cannot be queued a second time.
        assert_eq!(
            enqueue_job(&env, JobPayload::None, key(&env, 3, 1), 0, None).unwrap(),
            EnqueueOutcome::AlreadyCompleted(id)
        );
        assert_eq!(handler.calls(), 1);
        assert_eq!(job_stats(&env).counters.succeeded_total, 1);
    });
}

#[test]
fn failed_attempts_retry_with_exponential_backoff() {
    let env = Env::default();
    let (contract_id, _admin) = setup(&env);
    let handler = ScriptedHandler::new(1);

    env.as_contract(&contract_id, || {
        let worker = Address::generate(&env);
        let policy = RetryPolicy {
            max_attempts: 4,
            base_backoff_ledgers: 10,
            max_backoff_ledgers: 40,
        };
        let EnqueueOutcome::Created(id) =
            enqueue_job(&env, JobPayload::None, key(&env, 4, 1), 0, Some(policy)).unwrap()
        else {
            panic!("expected a new job")
        };

        let RunOutcome::Retried(job) = run_due_job(&env, &worker, 0, &handler).unwrap() else {
            panic!("expected a retry")
        };
        assert_eq!(job.attempts, 1);
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.last_error, 6, "Error::Expired");
        assert_eq!(job.next_attempt_ledger, 10, "now + base backoff");

        // The job is not due before its backoff has elapsed.
        assert_eq!(run_due_job(&env, &worker, 9, &handler).unwrap(), RunOutcome::Empty);
        assert_eq!(handler.calls(), 1);

        // Second attempt at ledger 10 succeeds; backoff doubles on each failure.
        let RunOutcome::Succeeded(job) = run_due_job(&env, &worker, 10, &handler).unwrap() else {
            panic!("expected the second attempt to succeed")
        };
        assert_eq!(job.attempts, 2);
        assert_eq!(job.last_attempt_ledger, 10);
        assert_eq!(handler.calls(), 2);
        assert_eq!(job_stats(&env).counters.failed_attempts_total, 1);
        assert_eq!(get_job(&env, id).unwrap().status, JobStatus::Succeeded);
    });
}

#[test]
fn retry_exhaustion_dead_letters_the_job_and_stops_running_it() {
    let env = Env::default();
    let (contract_id, _admin) = setup(&env);
    let handler = ScriptedHandler::new(99);

    env.as_contract(&contract_id, || {
        let worker = Address::generate(&env);
        let policy = RetryPolicy {
            max_attempts: 3,
            base_backoff_ledgers: 10,
            max_backoff_ledgers: 40,
        };
        let EnqueueOutcome::Created(id) =
            enqueue_job(&env, JobPayload::None, key(&env, 5, 1), 0, Some(policy)).unwrap()
        else {
            panic!("expected a new job")
        };

        let RunOutcome::Retried(first) = run_due_job(&env, &worker, 0, &handler).unwrap() else {
            panic!("expected a retry")
        };
        assert_eq!(first.next_attempt_ledger, 10);

        let RunOutcome::Retried(second) = run_due_job(&env, &worker, 10, &handler).unwrap() else {
            panic!("expected a second retry")
        };
        assert_eq!(second.attempts, 2);
        assert_eq!(second.next_attempt_ledger, 30, "now + 2 * base, capped at 40");

        let RunOutcome::DeadLettered(dead) = run_due_job(&env, &worker, 30, &handler).unwrap() else {
            panic!("expected the job to be dead-lettered")
        };
        assert_eq!(dead.status, JobStatus::DeadLettered);
        assert_eq!(dead.attempts, 3);
        assert_eq!(dead.last_error, 6);
        assert_eq!(dead.last_attempt_ledger, 30);
        assert_eq!(dead.worker, Some(worker.clone()));
        assert_eq!(dead.dedupe_key, key(&env, 5, 1));
        assert_eq!(handler.calls(), 3);

        assert_eq!(pending_job_ids(&env).len(), 0);
        assert_eq!(dead_letter_job_ids(&env).len(), 1);
        assert_eq!(job_stats(&env).counters.dead_lettered_total, 1);
        assert_eq!(job_stats(&env).counters.failed_attempts_total, 3);

        // A dead letter is never attempted again by the crank.
        assert_eq!(run_due_job(&env, &worker, 1_000, &handler).unwrap(), RunOutcome::Empty);
        assert_eq!(handler.calls(), 3);

        // It is not retryable until an admin requeues it.
        assert_eq!(
            reprocess_job(&env, &worker, id, 1_000, &handler),
            Err(JobError::NotRetryable)
        );
    });
}

#[test]
fn dead_letters_preserve_context_and_can_be_discarded() {
    let env = Env::default();
    let (contract_id, admin) = setup(&env);
    let handler = ScriptedHandler::new(99);

    env.as_contract(&contract_id, || {
        let worker = Address::generate(&env);
        let EnqueueOutcome::Created(id) =
            enqueue_job(&env, JobPayload::None, key(&env, 6, 1), 0, Some(RetryPolicy::no_retry()))
                .unwrap()
        else {
            panic!("expected a new job")
        };

        let RunOutcome::DeadLettered(_) = run_due_job(&env, &worker, 7, &handler).unwrap() else {
            panic!("expected a dead letter")
        };

        let job = get_job(&env, id).unwrap();
        assert_eq!(job.status, JobStatus::DeadLettered);
        assert_eq!(job.last_error, 6, "error code is kept for debugging");
        assert_eq!(job.last_attempt_ledger, 7);
        assert_eq!(job.attempts, 1);
        assert_eq!(job.worker, Some(worker));

        let discarded = discard_dead_letter(&env, &admin, id).unwrap();
        assert_eq!(discarded.id, id);
        assert_eq!(dead_letter_job_ids(&env).len(), 0);
        // The record survives for audit even after leaving the queue.
        assert_eq!(get_job(&env, id).unwrap().status, JobStatus::DeadLettered);
    });
}

#[test]
fn requeueing_a_dead_letter_restores_a_fresh_attempt_budget() {
    let env = Env::default();
    let (contract_id, admin) = setup(&env);
    let handler = ScriptedHandler::new(1);

    env.as_contract(&contract_id, || {
        let worker = Address::generate(&env);
        let EnqueueOutcome::Created(id) =
            enqueue_job(&env, JobPayload::None, key(&env, 7, 1), 0, Some(RetryPolicy::no_retry()))
                .unwrap()
        else {
            panic!("expected a new job")
        };

        let RunOutcome::DeadLettered(_) = run_due_job(&env, &worker, 0, &handler).unwrap() else {
            panic!("expected a dead letter")
        };
        assert_eq!(dead_letter_job_ids(&env).len(), 1);

        let requeued = requeue_dead_letter(&env, &admin, id, 50).unwrap();
        assert_eq!(requeued.status, JobStatus::Pending);
        assert_eq!(requeued.attempts, 0);
        assert_eq!(requeued.next_attempt_ledger, 50);
        assert_eq!(requeued.last_error, 0);
        assert_eq!(pending_job_ids(&env).len(), 1);
        assert_eq!(dead_letter_job_ids(&env).len(), 0);
        assert_eq!(next_due_ledger(&env), Some(50));

        let RunOutcome::Succeeded(job) = run_due_job(&env, &worker, 50, &handler).unwrap() else {
            panic!("expected the requeued job to succeed")
        };
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(handler.calls(), 2);
    });
}

#[test]
fn jobs_are_not_run_before_they_are_due() {
    let env = Env::default();
    let (contract_id, _admin) = setup(&env);
    let handler = ScriptedHandler::new(0);

    env.as_contract(&contract_id, || {
        let worker = Address::generate(&env);
        let EnqueueOutcome::Created(id) =
            enqueue_job(&env, JobPayload::None, key(&env, 8, 1), 100, None).unwrap()
        else {
            panic!("expected a new job")
        };

        assert_eq!(run_due_job(&env, &worker, 99, &handler).unwrap(), RunOutcome::Empty);
        assert_eq!(next_due_ledger(&env), Some(100));

        let RunOutcome::Skipped(job) = reprocess_job(&env, &worker, id, 99, &handler).unwrap() else {
            panic!("expected the job to be skipped")
        };
        assert_eq!(job.attempts, 0);
        assert_eq!(handler.calls(), 0);

        let RunOutcome::Succeeded(_) = run_due_job(&env, &worker, 100, &handler).unwrap() else {
            panic!("expected the job to run once due")
        };
        assert_eq!(handler.calls(), 1);
    });
}

#[test]
fn a_disabled_worker_refuses_new_work_and_runs() {
    let env = Env::default();
    let (contract_id, admin) = setup(&env);
    let handler = ScriptedHandler::new(0);
    let worker = Address::generate(&env);

    env.as_contract(&contract_id, || {
        let EnqueueOutcome::Created(_) =
            enqueue_job(&env, JobPayload::None, key(&env, 9, 1), 0, None).unwrap()
        else {
            panic!("expected a new job")
        };

        pause_worker(&env, &admin).unwrap();
        assert!(!worker_config(&env).enabled);
        assert_eq!(
            run_due_job(&env, &worker, 0, &handler),
            Err(JobError::Disabled)
        );
        assert_eq!(
            enqueue_job(&env, JobPayload::None, key(&env, 10, 1), 0, None),
            Err(JobError::Disabled)
        );
        assert_eq!(handler.calls(), 0);
    });

    // A separate contract frame: the admin re-enables the worker and the job
    // that was queued before the pause still runs.
    env.as_contract(&contract_id, || {
        resume_worker(&env, &admin).unwrap();
        assert!(worker_config(&env).enabled);
        let RunOutcome::Succeeded(_) = run_due_job(&env, &worker, 0, &handler).unwrap() else {
            panic!("expected the job to run once resumed")
        };
        assert_eq!(handler.calls(), 1);
    });
}

#[test]
fn invalid_retry_policies_are_rejected() {
    let env = Env::default();
    let (contract_id, admin) = setup(&env);

    env.as_contract(&contract_id, || {
        let no_attempts = RetryPolicy {
            max_attempts: 0,
            base_backoff_ledgers: 1,
            max_backoff_ledgers: 1,
        };
        let inverted_cap = RetryPolicy {
            max_attempts: 2,
            base_backoff_ledgers: 50,
            max_backoff_ledgers: 10,
        };

        assert_eq!(
            configure_worker(
                &env,
                &admin,
                WorkerConfig {
                    enabled: true,
                    policy: no_attempts,
                },
            ),
            Err(JobError::InvalidPolicy)
        );
        assert_eq!(
            enqueue_job(&env, JobPayload::None, key(&env, 11, 1), 0, Some(inverted_cap)),
            Err(JobError::InvalidPolicy)
        );
        // The config is untouched by the rejected update.
        assert_eq!(worker_config(&env).policy, RetryPolicy::maintenance());
    });
}

#[test]
fn only_the_admin_can_manage_the_worker() {
    let env = Env::default();
    let (contract_id, _admin) = setup(&env);

    env.as_contract(&contract_id, || {
        let stranger = Address::generate(&env);
        let EnqueueOutcome::Created(id) =
            enqueue_job(&env, JobPayload::None, key(&env, 12, 1), 0, Some(RetryPolicy::no_retry()))
                .unwrap()
        else {
            panic!("expected a new job")
        };
        let handler = ScriptedHandler::new(99);
        let worker = Address::generate(&env);
        let RunOutcome::DeadLettered(_) = run_due_job(&env, &worker, 0, &handler).unwrap() else {
            panic!("expected a dead letter")
        };

        assert_eq!(pause_worker(&env, &stranger), Err(JobError::Unauthorized));
        assert_eq!(
            requeue_dead_letter(&env, &stranger, id, 0),
            Err(JobError::Unauthorized)
        );
        assert_eq!(
            discard_dead_letter(&env, &stranger, id),
            Err(JobError::Unauthorized)
        );
        assert_eq!(dead_letter_job_ids(&env).len(), 1);
    });
}

#[test]
fn escrow_refund_jobs_carry_their_payload_and_are_deduped_by_escrow_id() {
    let env = Env::default();
    let (contract_id, _admin) = setup(&env);

    env.as_contract(&contract_id, || {
        let token = Address::generate(&env);
        let EnqueueOutcome::Created(id) = enqueue_escrow_refund(&env, token.clone(), 42, 0).unwrap()
        else {
            panic!("expected a new job")
        };
        assert_eq!(
            enqueue_escrow_refund(&env, token.clone(), 42, 0).unwrap(),
            EnqueueOutcome::AlreadyPending(id)
        );
        // A different escrow is a different job.
        let EnqueueOutcome::Created(other) = enqueue_escrow_refund(&env, token.clone(), 43, 0).unwrap()
        else {
            panic!("expected a second job")
        };
        assert_ne!(id, other);

        let job = get_job(&env, id).unwrap();
        assert_eq!(job.kind, JobKind::EscrowRefund);
        assert_eq!(job.kind.as_symbol(), symbol_short!("esc_ref"));
        assert_eq!(job.payload, JobPayload::EscrowRefund(token.clone(), 42));
        assert_eq!(job_stats(&env).pending, 2);
        assert_eq!(job_stats(&env).next_due_ledger, Some(0));
    });
}

#[test]
fn unknown_job_ids_are_reported() {
    let env = Env::default();
    let (contract_id, _admin) = setup(&env);
    let handler = ScriptedHandler::new(0);

    env.as_contract(&contract_id, || {
        let worker = Address::generate(&env);
        assert_eq!(get_job(&env, 404), None);
        assert_eq!(
            reprocess_job(&env, &worker, 404, 0, &handler),
            Err(JobError::NotFound)
        );
    });
}

// ---------------------------------------------------------------------------
// Escrow refunds — the delayed operation moved into the worker framework
// ---------------------------------------------------------------------------

/// Mints a test asset and returns `(token, depositor)`.
fn funded_token(env: &Env, amount: i128) -> (Address, Address) {
    let token_admin = Address::generate(env);
    let token = env.register_stellar_asset_contract(token_admin);
    let depositor = Address::generate(env);
    token::StellarAssetClient::new(env, &token).mint(&depositor, &amount);
    (token, depositor)
}

#[test]
fn creating_an_escrow_with_a_refund_job_defers_the_refund_until_expiry() {
    let env = Env::default();
    let (contract_id, _admin) = setup(&env);
    let (token, depositor) = funded_token(&env, 5_000);
    let beneficiary = Address::generate(&env);
    let expiry = env.ledger().sequence() + 5;

    // Creation goes through a contract invocation so the depositor's
    // authorization is rooted in the call, exactly as a payments contract
    // would drive it.
    let (escrow_id, outcome) = JobFixtureClient::new(&env, &contract_id)
        .create_escrow_schedule_refund(&token, &depositor, &beneficiary, &2_000, &expiry);
    let EnqueueOutcome::Created(job_id) = outcome else {
        panic!("expected a new refund job")
    };

    env.as_contract(&contract_id, || {
        let job = get_job(&env, job_id).unwrap();
        assert_eq!(job.kind, JobKind::EscrowRefund);
        assert_eq!(
            job.due_ledger, expiry,
            "the refund is not attempted before the deposit expires"
        );
        assert_eq!(job.payload, JobPayload::EscrowRefund(token.clone(), escrow_id));

        // Re-scheduling the same escrow is deduped by escrow id.
        assert_eq!(
            schedule_escrow_refund(&env, &token, escrow_id, expiry).unwrap(),
            EnqueueOutcome::AlreadyPending(job_id)
        );

        let worker = Address::generate(&env);
        assert_eq!(
            run_due_job(&env, &worker, expiry - 1, &EscrowRefundHandler).unwrap(),
            RunOutcome::Empty
        );
        assert_eq!(get_escrow(&env, escrow_id).unwrap().state, EscrowState::Active);

        let RunOutcome::Succeeded(job) =
            run_due_job(&env, &worker, expiry, &EscrowRefundHandler).unwrap()
        else {
            panic!("expected the refund to succeed at expiry")
        };
        assert_eq!(job.attempts, 1);
        assert_eq!(job.completed_ledger, expiry);
        assert_eq!(get_escrow(&env, escrow_id).unwrap().state, EscrowState::Refunded);
        assert_eq!(pending_job_ids(&env).len(), 0);

        // The finished refund is never scheduled again.
        assert_eq!(
            schedule_escrow_refund(&env, &token, escrow_id, expiry).unwrap(),
            EnqueueOutcome::AlreadyCompleted(job_id)
        );
    });
}

#[test]
fn an_escrow_released_before_expiry_dead_letters_its_refund_job() {
    let env = Env::default();
    let (contract_id, _admin) = setup(&env);
    let (token, depositor) = funded_token(&env, 5_000);
    let beneficiary = Address::generate(&env);
    let expiry = env.ledger().sequence() + 5;

    let (escrow_id, outcome) = JobFixtureClient::new(&env, &contract_id)
        .create_escrow_schedule_refund(&token, &depositor, &beneficiary, &2_000, &expiry);
    let EnqueueOutcome::Created(job_id) = outcome else {
        panic!("expected a new refund job")
    };

    env.as_contract(&contract_id, || {
        // The beneficiary is paid out first, which makes the scheduled refund
        // impossible: the handler will fail on every attempt.
        release_escrow(&env, &token, escrow_id).unwrap();
        assert_eq!(get_escrow(&env, escrow_id).unwrap().state, EscrowState::Released);

        let worker = Address::generate(&env);
        let policy = worker_config(&env).policy;
        assert_eq!(policy.max_attempts, 5);

        let mut now = expiry;
        for attempt in 1..policy.max_attempts {
            let RunOutcome::Retried(job) =
                run_due_job(&env, &worker, now, &EscrowRefundHandler).unwrap()
            else {
                panic!("expected attempt {attempt} to be retried")
            };
            assert_eq!(job.attempts, attempt);
            assert_eq!(job.last_error, 703, "Error::PaymentEscrowAlreadyReleased");
            now = now.saturating_add(policy.backoff_after(attempt));
            assert_eq!(job.next_attempt_ledger, now);
        }

        let RunOutcome::DeadLettered(job) =
            run_due_job(&env, &worker, now, &EscrowRefundHandler).unwrap()
        else {
            panic!("expected the refund job to be dead-lettered")
        };
        assert_eq!(job.attempts, policy.max_attempts);
        assert_eq!(job.last_error, 703);
        assert_eq!(job.status, JobStatus::DeadLettered);
        assert_eq!(job.worker, Some(worker));
        assert_eq!(
            job.dedupe_key,
            dedupe_key_u64(&env, ESCROW_REFUND_TAG, escrow_id)
        );

        assert_eq!(dead_letter_job_ids(&env).len(), 1);
        assert_eq!(pending_job_ids(&env).len(), 0);
        // The deposit stays released; nothing was double-refunded.
        assert_eq!(get_escrow(&env, escrow_id).unwrap().state, EscrowState::Released);
        assert_eq!(get_job(&env, job_id).unwrap().status, JobStatus::DeadLettered);
    });
}
