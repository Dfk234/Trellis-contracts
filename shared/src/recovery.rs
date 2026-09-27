//! Deterministic recovery for interrupted multi-step operations (Issue #49).
//!
//! Multi-step operations (a wallet action, an API call, a webhook delivery, a
//! worker job, a browser session) can fail after some steps have already had
//! **external side effects**. Blindly retrying the whole operation duplicates
//! those side effects; refusing to retry strands the caller.
//!
//! This module records a [`RecoveryCheckpoint`] per operation:
//!
//! - **Deterministic ordering.** Steps are completed strictly in order via
//!   [`complete_step`], and `cursor` always points at the next step to run. A
//!   resumed run therefore continues at the same point, never skipping or
//!   repeating a step.
//! - **Idempotent completion.** Completing a step that already succeeded is a
//!   no-op that returns the current checkpoint, so a retried external side
//!   effect is not applied twice.
//! - **Safe failure.** [`fail_step`] moves the operation to `AwaitingRecovery`
//!   and records the failing step + error code; [`resume`] retries from the
//!   cursor; [`abandon`] fails the operation without further side effects.
//! - **User-visible next steps.** [`next_action`] returns a short symbol the
//!   caller can surface (`run`, `resume`, `done`, `support`).
//! - **Maintainer diagnostics.** [`diagnostics`] summarizes progress and
//!   [`is_stuck`] flags operations that have not advanced within a threshold.
//!
//! See `docs/RECOVERY.md` for the state machine and operator runbook.

use soroban_sdk::{contracterror, contracttype, symbol_short, Env, Symbol, Vec};

use crate::storage::{persistent_get, persistent_set};

/// Operation lifecycle state.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationState {
    /// The operation has not started running yet.
    NotStarted,
    /// Steps are being executed.
    InProgress,
    /// A step failed and the operation needs to be resumed or abandoned.
    AwaitingRecovery,
    /// Every step completed successfully.
    Completed,
    /// The operation was abandoned or permanently failed.
    Failed,
}

/// Kind of caller driving the operation, for diagnostics.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    /// A wallet-signed action.
    WalletAction,
    /// A backend API call.
    ApiCall,
    /// A webhook delivery.
    Webhook,
    /// A background worker job.
    Worker,
    /// A browser session.
    BrowserSession,
    /// An on-chain contract call.
    ContractCall,
}

/// Outcome of a single step.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepOutcome {
    /// Not attempted yet.
    Pending,
    /// Completed successfully.
    Succeeded,
    /// Attempted and failed; the operation needs recovery.
    Failed,
    /// Intentionally skipped.
    Skipped,
}

/// One step in the multi-step operation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryStep {
    /// Zero-based position; must be strictly increasing across the operation.
    pub index: u32,
    /// Short step name (e.g. `"debit"`, `"notify"`).
    pub name: Symbol,
    /// `true` when this step performs an external side effect that must not be
    /// repeated on resume once it has succeeded.
    pub external: bool,
    /// Current outcome of this step.
    pub outcome: StepOutcome,
    /// Number of failed attempts recorded for this step.
    pub attempts: u32,
}

/// Persisted recovery checkpoint for one operation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryCheckpoint {
    /// Operation identifier (unique per in-flight operation).
    pub id: Symbol,
    /// Caller kind, for diagnostics.
    pub kind: OperationKind,
    /// Current lifecycle state.
    pub state: OperationState,
    /// Ordered steps.
    pub steps: Vec<RecoveryStep>,
    /// Index of the next step to run. Always the deterministic resume point.
    pub cursor: u32,
    /// Total failed attempts recorded against the operation.
    pub attempts: u32,
    /// Ledger timestamp when the operation started.
    pub started_at: u64,
    /// Ledger timestamp of the last update.
    pub updated_at: u64,
    /// Last error code reported by [`fail_step`] (0 when none).
    pub last_error: u32,
    /// Short, user-visible next action (`run`, `resume`, `done`, `support`).
    pub next_action: Symbol,
}

/// Progress summary for maintainers.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryDiagnostics {
    /// Current lifecycle state.
    pub state: OperationState,
    /// Next step index.
    pub cursor: u32,
    /// Total steps.
    pub total: u32,
    /// Steps that succeeded.
    pub succeeded: u32,
    /// Steps that failed.
    pub failed: u32,
    /// Steps still pending or skipped.
    pub pending: u32,
    /// Attempts recorded against the operation.
    pub attempts: u32,
    /// Last error code (0 when none).
    pub last_error: u32,
    /// Short next action for the caller.
    pub next_action: Symbol,
}

/// Recovery operation errors (range 940–959).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum RecoveryError {
    /// An operation with this id already exists and is not finished.
    OperationExists = 940,
    /// No checkpoint exists for this operation id.
    OperationNotFound = 941,
    /// The operation has already completed.
    AlreadyCompleted = 942,
    /// A step was completed or failed out of order.
    StepOutOfOrder = 943,
    /// The operation is in a state that does not allow this transition.
    WrongState = 944,
    /// The step list is empty or has non-contiguous indices.
    InvalidSteps = 945,
}

/// Short user-visible actions.
pub const ACTION_RUN: Symbol = symbol_short!("run");
pub const ACTION_RESUME: Symbol = symbol_short!("resume");
pub const ACTION_DONE: Symbol = symbol_short!("done");
pub const ACTION_SUPPORT: Symbol = symbol_short!("support");
pub const ACTION_UNKNOWN: Symbol = symbol_short!("unknown");

/// Storage key for one operation checkpoint.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum RecoveryKey {
    Op(Symbol),
}

fn key(id: &Symbol) -> RecoveryKey {
    RecoveryKey::Op(id.clone())
}

/// Load a checkpoint, if one exists.
pub fn load(env: &Env, id: &Symbol) -> Option<RecoveryCheckpoint> {
    persistent_get(env, &key(id))
}

/// Persist a checkpoint.
pub fn save(env: &Env, checkpoint: &RecoveryCheckpoint) {
    persistent_set(env, &key(&checkpoint.id), checkpoint);
}

/// Validate that step indices are non-empty and contiguous from zero.
fn validate_steps(steps: &Vec<RecoveryStep>) -> Result<(), RecoveryError> {
    if steps.is_empty() {
        return Err(RecoveryError::InvalidSteps);
    }
    let mut expected: u32 = 0;
    for step in steps.iter() {
        if step.index != expected {
            return Err(RecoveryError::InvalidSteps);
        }
        expected = expected.saturating_add(1);
    }
    Ok(())
}

/// Rebuild a step list with one step's outcome (and optionally attempts)
/// updated. Rebuilding keeps the update total and avoids in-place mutation
/// edge cases across Soroban `Vec` handles.
fn with_step(
    env: &Env,
    steps: &Vec<RecoveryStep>,
    index: u32,
    outcome: StepOutcome,
    inc_attempts: bool,
) -> Vec<RecoveryStep> {
    let mut out = Vec::new(env);
    let mut i: u32 = 0;
    for step in steps.iter() {
        let mut step = step;
        if i == index {
            step.outcome = outcome;
            if inc_attempts {
                step.attempts = step.attempts.saturating_add(1);
            }
        }
        out.push_back(step);
        i = i.saturating_add(1);
    }
    out
}

/// Open a new operation. Fails while a previous attempt with the same id is
/// still in flight, so a duplicate submit cannot fork the step list.
pub fn open_operation(
    env: &Env,
    id: &Symbol,
    kind: OperationKind,
    steps: Vec<RecoveryStep>,
    at: u64,
) -> Result<RecoveryCheckpoint, RecoveryError> {
    validate_steps(&steps)?;
    if let Some(existing) = load(env, id) {
        if existing.state != OperationState::Completed && existing.state != OperationState::Failed {
            return Err(RecoveryError::OperationExists);
        }
    }
    let checkpoint = RecoveryCheckpoint {
        id: id.clone(),
        kind,
        state: OperationState::InProgress,
        steps,
        cursor: 0,
        attempts: 0,
        started_at: at,
        updated_at: at,
        last_error: 0,
        next_action: ACTION_RUN,
    };
    save(env, &checkpoint);
    Ok(checkpoint)
}

/// Mark the step at `cursor` complete.
///
/// Out-of-order indices are rejected. Completing a step that already succeeded
/// is an idempotent no-op (no duplicate side effect) and returns the current
/// checkpoint.
pub fn complete_step(
    env: &Env,
    id: &Symbol,
    index: u32,
    at: u64,
) -> Result<RecoveryCheckpoint, RecoveryError> {
    let mut checkpoint = load(env, id).ok_or(RecoveryError::OperationNotFound)?;
    match checkpoint.state {
        OperationState::Completed => return Err(RecoveryError::AlreadyCompleted),
        OperationState::Failed => return Err(RecoveryError::WrongState),
        OperationState::AwaitingRecovery | OperationState::InProgress | OperationState::NotStarted => {}
    }
    if index != checkpoint.cursor {
        return Err(RecoveryError::StepOutOfOrder);
    }
    let step = checkpoint
        .steps
        .get(index)
        .ok_or(RecoveryError::StepOutOfOrder)?;
    if step.outcome == StepOutcome::Succeeded {
        // Idempotent: already done, do not re-run the side effect.
        return Ok(checkpoint);
    }
    checkpoint.steps = with_step(env, &checkpoint.steps, index, StepOutcome::Succeeded, false);
    checkpoint.cursor = checkpoint.cursor.saturating_add(1);
    checkpoint.updated_at = at;
    if checkpoint.cursor >= checkpoint.steps.len() {
        checkpoint.state = OperationState::Completed;
        checkpoint.next_action = ACTION_DONE;
    } else {
        checkpoint.state = OperationState::InProgress;
    }
    save(env, &checkpoint);
    Ok(checkpoint)
}

/// Record a failure at the step at `cursor` and move to `AwaitingRecovery`.
pub fn fail_step(
    env: &Env,
    id: &Symbol,
    index: u32,
    error_code: u32,
    at: u64,
) -> Result<RecoveryCheckpoint, RecoveryError> {
    let mut checkpoint = load(env, id).ok_or(RecoveryError::OperationNotFound)?;
    match checkpoint.state {
        OperationState::Completed => return Err(RecoveryError::AlreadyCompleted),
        OperationState::Failed => return Err(RecoveryError::WrongState),
        _ => {}
    }
    if index != checkpoint.cursor {
        return Err(RecoveryError::StepOutOfOrder);
    }
    if checkpoint.steps.get(index).is_none() {
        return Err(RecoveryError::StepOutOfOrder);
    }
    checkpoint.steps = with_step(env, &checkpoint.steps, index, StepOutcome::Failed, true);
    checkpoint.state = OperationState::AwaitingRecovery;
    checkpoint.last_error = error_code;
    checkpoint.attempts = checkpoint.attempts.saturating_add(1);
    checkpoint.next_action = ACTION_RESUME;
    checkpoint.updated_at = at;
    save(env, &checkpoint);
    Ok(checkpoint)
}

/// Resume an interrupted operation from the cursor.
///
/// The failed step is reset to `Pending` and will be retried; steps that
/// already succeeded are left `Succeeded` and are never re-run.
pub fn resume(env: &Env, id: &Symbol, at: u64) -> Result<RecoveryCheckpoint, RecoveryError> {
    let mut checkpoint = load(env, id).ok_or(RecoveryError::OperationNotFound)?;
    match checkpoint.state {
        OperationState::Completed => return Err(RecoveryError::AlreadyCompleted),
        OperationState::Failed => return Err(RecoveryError::WrongState),
        _ => {}
    }
    if checkpoint.cursor >= checkpoint.steps.len() {
        // Nothing left to run; treat it as complete rather than resuming.
        checkpoint.state = OperationState::Completed;
        checkpoint.next_action = ACTION_DONE;
        checkpoint.updated_at = at;
        save(env, &checkpoint);
        return Ok(checkpoint);
    }
    let failed = checkpoint
        .steps
        .get(checkpoint.cursor)
        .map(|s| s.outcome == StepOutcome::Failed)
        .unwrap_or(false);
    if failed {
        checkpoint.steps = with_step(env, &checkpoint.steps, checkpoint.cursor, StepOutcome::Pending, false);
    }
    checkpoint.state = OperationState::InProgress;
    checkpoint.next_action = ACTION_RESUME;
    checkpoint.updated_at = at;
    save(env, &checkpoint);
    Ok(checkpoint)
}

/// Abandon an interrupted operation without further side effects.
pub fn abandon(env: &Env, id: &Symbol, at: u64) -> Result<RecoveryCheckpoint, RecoveryError> {
    let mut checkpoint = load(env, id).ok_or(RecoveryError::OperationNotFound)?;
    if checkpoint.state == OperationState::Completed {
        return Err(RecoveryError::AlreadyCompleted);
    }
    checkpoint.state = OperationState::Failed;
    checkpoint.next_action = ACTION_SUPPORT;
    checkpoint.updated_at = at;
    save(env, &checkpoint);
    Ok(checkpoint)
}

/// Short, user-visible next action for the operation.
pub fn next_action(env: &Env, id: &Symbol) -> Symbol {
    match load(env, id) {
        Some(checkpoint) => checkpoint.next_action,
        None => ACTION_UNKNOWN,
    }
}

/// Maintainer diagnostics for an operation.
pub fn diagnostics(env: &Env, id: &Symbol) -> Option<RecoveryDiagnostics> {
    let checkpoint = load(env, id)?;
    let mut succeeded: u32 = 0;
    let mut failed: u32 = 0;
    let mut pending: u32 = 0;
    for step in checkpoint.steps.iter() {
        match step.outcome {
            StepOutcome::Succeeded => succeeded = succeeded.saturating_add(1),
            StepOutcome::Failed => failed = failed.saturating_add(1),
            StepOutcome::Pending | StepOutcome::Skipped => pending = pending.saturating_add(1),
        }
    }
    Some(RecoveryDiagnostics {
        state: checkpoint.state,
        cursor: checkpoint.cursor,
        total: checkpoint.steps.len(),
        succeeded,
        failed,
        pending,
        attempts: checkpoint.attempts,
        last_error: checkpoint.last_error,
        next_action: checkpoint.next_action,
    })
}

/// Whether an in-progress operation has not advanced for `stale_after` units.
pub fn is_stuck(env: &Env, id: &Symbol, now: u64, stale_after: u64) -> bool {
    match load(env, id) {
        Some(checkpoint) => {
            checkpoint.state == OperationState::InProgress
                && now.saturating_sub(checkpoint.updated_at) >= stale_after
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(env: &Env, external: &[bool]) -> Vec<RecoveryStep> {
        let mut out = Vec::new(env);
        let mut i: u32 = 0;
        for ext in external {
            out.push_back(RecoveryStep {
                index: i,
                name: symbol_short!("step"),
                external: *ext,
                outcome: StepOutcome::Pending,
                attempts: 0,
            });
            i = i.saturating_add(1);
        }
        out
    }

    #[test]
    fn completes_in_order_and_completion_is_idempotent() {
        let env = Env::default();
        let id = symbol_short!("op1");
        let mut cp = open_operation(&env, &id, OperationKind::ApiCall, steps(&env, &[false, false, false]), 1).unwrap();
        assert_eq!(cp.state, OperationState::InProgress);
        assert_eq!(cp.cursor, 0);

        cp = complete_step(&env, &id, 0, 2).unwrap();
        assert_eq!(cp.cursor, 1);

        // Re-completing a succeeded step is a no-op (no duplicate side effect).
        let again = complete_step(&env, &id, 0, 3).unwrap();
        assert_eq!(again.cursor, 1);
        assert_eq!(again.steps.get(0).unwrap().attempts, 0);

        cp = complete_step(&env, &id, 1, 4).unwrap();
        cp = complete_step(&env, &id, 2, 5).unwrap();
        assert_eq!(cp.state, OperationState::Completed);
        assert_eq!(cp.next_action, ACTION_DONE);
        assert_eq!(next_action(&env, &id), ACTION_DONE);
        // Finishing again is rejected.
        assert_eq!(
            complete_step(&env, &id, 2, 6),
            Err(RecoveryError::AlreadyCompleted)
        );
    }

    #[test]
    fn out_of_order_and_invalid_steps_are_rejected() {
        let env = Env::default();
        let id = symbol_short!("op2");
        open_operation(&env, &id, OperationKind::Worker, steps(&env, &[false, false]), 1).unwrap();
        assert_eq!(
            complete_step(&env, &id, 1, 2),
            Err(RecoveryError::StepOutOfOrder)
        );
        assert_eq!(fail_step(&env, &id, 1, 99, 2), Err(RecoveryError::StepOutOfOrder));

        // Empty / non-contiguous step lists are rejected up front.
        let empty: Vec<RecoveryStep> = Vec::new(&env);
        assert_eq!(
            open_operation(&env, &symbol_short!("op3"), OperationKind::Worker, empty, 1),
            Err(RecoveryError::InvalidSteps)
        );
        let mut bad = Vec::new(&env);
        bad.push_back(RecoveryStep {
            index: 1,
            name: symbol_short!("x"),
            external: false,
            outcome: StepOutcome::Pending,
            attempts: 0,
        });
        assert_eq!(
            open_operation(&env, &symbol_short!("op4"), OperationKind::Worker, bad, 1),
            Err(RecoveryError::InvalidSteps)
        );
    }

    #[test]
    fn interruption_at_external_side_effect_resumes_without_duplicate() {
        let env = Env::default();
        let id = symbol_short!("op5");
        // Step 0 and 1 have external side effects (e.g. debit + notify).
        open_operation(&env, &id, OperationKind::WalletAction, steps(&env, &[true, true, false]), 10).unwrap();

        let cp = complete_step(&env, &id, 0, 11).unwrap();
        assert_eq!(cp.cursor, 1);

        // Interruption during the second external step.
        let failed = fail_step(&env, &id, 1, 4242, 12).unwrap();
        assert_eq!(failed.state, OperationState::AwaitingRecovery);
        assert_eq!(failed.last_error, 4242);
        assert_eq!(failed.next_action, ACTION_RESUME);

        // Resume retries only the failed step.
        let resumed = resume(&env, &id, 13).unwrap();
        assert_eq!(resumed.state, OperationState::InProgress);
        assert_eq!(resumed.cursor, 1);
        assert_eq!(resumed.steps.get(1).unwrap().outcome, StepOutcome::Pending);

        let cp = complete_step(&env, &id, 1, 14).unwrap();
        let cp = complete_step(&env, &id, 2, 15).unwrap();
        assert_eq!(cp.state, OperationState::Completed);
        // The already-succeeded external step was never retried.
        assert_eq!(cp.steps.get(0).unwrap().attempts, 0);
        assert_eq!(cp.steps.get(1).unwrap().attempts, 1);
    }

    #[test]
    fn diagnostics_and_next_action_are_reported() {
        let env = Env::default();
        let id = symbol_short!("op6");
        open_operation(&env, &id, OperationKind::Webhook, steps(&env, &[false, false, false]), 1).unwrap();
        complete_step(&env, &id, 0, 2).unwrap();
        fail_step(&env, &id, 1, 7, 3).unwrap();

        let d = diagnostics(&env, &id).unwrap();
        assert_eq!(d.state, OperationState::AwaitingRecovery);
        assert_eq!(d.total, 3);
        assert_eq!(d.succeeded, 1);
        assert_eq!(d.failed, 1);
        assert_eq!(d.pending, 1);
        assert_eq!(d.cursor, 1);
        assert_eq!(d.attempts, 1);
        assert_eq!(d.last_error, 7);
        assert_eq!(d.next_action, ACTION_RESUME);

        // Unknown operations report a support action rather than panicking.
        assert_eq!(next_action(&env, &symbol_short!("nope")), ACTION_UNKNOWN);
        assert!(diagnostics(&env, &symbol_short!("nope")).is_none());
    }

    #[test]
    fn abandon_marks_failed_and_stuck_detection_works() {
        let env = Env::default();
        let id = symbol_short!("op7");
        open_operation(&env, &id, OperationKind::BrowserSession, steps(&env, &[false, false]), 100).unwrap();

        // In progress at t=130 with a 25-unit staleness window -> stuck.
        assert!(!is_stuck(&env, &id, 120, 25));
        assert!(is_stuck(&env, &id, 130, 25));

        let abandoned = abandon(&env, &id, 140).unwrap();
        assert_eq!(abandoned.state, OperationState::Failed);
        assert_eq!(abandoned.next_action, ACTION_SUPPORT);
        // Failed operations are no longer "stuck" (nothing left to resume).
        assert!(!is_stuck(&env, &id, 200, 1));
        assert_eq!(complete_step(&env, &id, 0, 141), Err(RecoveryError::WrongState));

        // A finished operation id can be reopened for a fresh attempt.
        assert!(open_operation(&env, &id, OperationKind::BrowserSession, steps(&env, &[false]), 150).is_ok());
    }

    #[test]
    fn in_flight_operation_cannot_be_reopened() {
        let env = Env::default();
        let id = symbol_short!("op8");
        open_operation(&env, &id, OperationKind::ContractCall, steps(&env, &[false]), 1).unwrap();
        assert_eq!(
            open_operation(&env, &id, OperationKind::ContractCall, steps(&env, &[false]), 2),
            Err(RecoveryError::OperationExists)
        );
        assert!(load(&env, &symbol_short!("missing")).is_none());
    }
}
