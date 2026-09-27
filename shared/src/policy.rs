//! Central policy engine for configurable business rules (Issue #55).
//!
//! Business limits, eligibility thresholds, and restrictions that were
//! previously hard-coded across handlers now flow through one place:
//!
//! - [`PolicyConfig`] — the maintainer-tunable rules.
//! - [`PolicyInput`] — typed facts about a request.
//! - [`evaluate`] — pure, deterministic decision returning a [`PolicyDecision`]
//!   with a machine-readable [`PolicyReason`].
//! - [`require_policy`] — the guard contracts call at entry points; maps the
//!   decision onto stable, user-safe [`Error`] values.
//!
//! ## Default policy
//!
//! | Rule | Default |
//! |---|---:|
//! | Minimum amount | `1` |
//! | Maximum amount | `1_000_000_000` |
//! | Max operations per day | `50` |
//! | Highest allowed actor tier | `3` |
//! | Region restriction | disabled |
//!
//! Boundaries are inclusive: an amount equal to `min_amount` / `max_amount`
//! passes, and `daily_ops == max_daily_ops` is rejected (the limit is the
//! maximum number of *completed* operations).

use soroban_sdk::contracttype;

use crate::errors::Error;

/// Default maximum single-operation amount.
pub const DEFAULT_MAX_AMOUNT: i128 = 1_000_000_000;
/// Default minimum single-operation amount.
pub const DEFAULT_MIN_AMOUNT: i128 = 1;
/// Default daily operation cap per actor.
pub const DEFAULT_MAX_DAILY_OPS: u32 = 50;
/// Default highest actor tier allowed by policy.
pub const DEFAULT_ALLOWED_MAX_TIER: u32 = 3;

/// Maintainer-tunable business rules.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyConfig {
    pub min_amount: i128,
    pub max_amount: i128,
    pub max_daily_ops: u32,
    pub allowed_max_tier: u32,
    pub region_allowed: bool,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            min_amount: DEFAULT_MIN_AMOUNT,
            max_amount: DEFAULT_MAX_AMOUNT,
            max_daily_ops: DEFAULT_MAX_DAILY_OPS,
            allowed_max_tier: DEFAULT_ALLOWED_MAX_TIER,
            region_allowed: true,
        }
    }
}

/// Typed facts about a request, supplied by the calling contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyInput {
    pub amount: i128,
    pub actor_tier: u32,
    pub daily_ops: u32,
    pub region_allowed: bool,
}

/// Machine-readable outcome of a policy evaluation.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyReason {
    Allowed,
    AmountBelowMinimum,
    AmountAboveMaximum,
    DailyLimitReached,
    TierNotAllowed,
    RegionRestricted,
    InvalidConfig,
}

/// Decision returned by [`evaluate`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyDecision {
    pub allowed: bool,
    pub reason: PolicyReason,
}

/// Validate a policy configuration independently of any request.
pub fn validate_policy(config: &PolicyConfig) -> Result<(), Error> {
    if config.min_amount <= 0 || config.max_amount < config.min_amount || config.max_daily_ops == 0
    {
        return Err(Error::ConfigInvalid);
    }
    Ok(())
}

/// Evaluate a request against the policy. Pure — no storage, no ledger reads.
pub fn evaluate(config: &PolicyConfig, input: &PolicyInput) -> PolicyDecision {
    let deny = |reason: PolicyReason| PolicyDecision {
        allowed: false,
        reason,
    };

    if validate_policy(config).is_err() {
        return deny(PolicyReason::InvalidConfig);
    }
    if input.amount < config.min_amount {
        return deny(PolicyReason::AmountBelowMinimum);
    }
    if input.amount > config.max_amount {
        return deny(PolicyReason::AmountAboveMaximum);
    }
    if input.actor_tier > config.allowed_max_tier {
        return deny(PolicyReason::TierNotAllowed);
    }
    if !config.region_allowed && !input.region_allowed {
        return deny(PolicyReason::RegionRestricted);
    }
    if input.daily_ops >= config.max_daily_ops {
        return deny(PolicyReason::DailyLimitReached);
    }

    PolicyDecision {
        allowed: true,
        reason: PolicyReason::Allowed,
    }
}

/// Guard for contract entry points: `Ok(())` when allowed, otherwise a stable,
/// user-safe error that reveals no internal state.
pub fn require_policy(config: &PolicyConfig, input: &PolicyInput) -> Result<(), Error> {
    let decision = evaluate(config, input);
    if decision.allowed {
        return Ok(());
    }
    Err(match decision.reason {
        PolicyReason::AmountBelowMinimum | PolicyReason::AmountAboveMaximum => Error::InvalidAmount,
        PolicyReason::DailyLimitReached => Error::WithdrawalLimitExceeded,
        PolicyReason::TierNotAllowed | PolicyReason::RegionRestricted => Error::Unauthorized,
        PolicyReason::InvalidConfig => Error::ConfigInvalid,
        PolicyReason::Allowed => return Ok(()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(amount: i128) -> PolicyInput {
        PolicyInput {
            amount,
            actor_tier: 1,
            daily_ops: 0,
            region_allowed: true,
        }
    }

    #[test]
    fn default_policy_allows_typical_request() {
        let decision = evaluate(&PolicyConfig::default(), &input(100));
        assert!(decision.allowed);
        assert_eq!(decision.reason, PolicyReason::Allowed);
        assert!(require_policy(&PolicyConfig::default(), &input(100)).is_ok());
    }

    #[test]
    fn amount_boundaries_are_inclusive() {
        let cfg = PolicyConfig::default();
        assert!(evaluate(&cfg, &input(cfg.min_amount)).allowed);
        assert!(evaluate(&cfg, &input(cfg.max_amount)).allowed);
        assert_eq!(
            evaluate(&cfg, &input(cfg.min_amount - 1)).reason,
            PolicyReason::AmountBelowMinimum
        );
        assert_eq!(
            evaluate(&cfg, &input(cfg.max_amount + 1)).reason,
            PolicyReason::AmountAboveMaximum
        );
    }

    #[test]
    fn tier_and_region_rules_apply() {
        let cfg = PolicyConfig {
            allowed_max_tier: 2,
            region_allowed: false,
            ..PolicyConfig::default()
        };
        let mut i = input(10);
        i.actor_tier = 3;
        assert_eq!(evaluate(&cfg, &i).reason, PolicyReason::TierNotAllowed);
        assert_eq!(require_policy(&cfg, &i), Err(Error::Unauthorized));

        let mut j = input(10);
        j.region_allowed = false;
        assert_eq!(evaluate(&cfg, &j).reason, PolicyReason::RegionRestricted);
    }

    #[test]
    fn daily_limit_is_enforced_at_boundary() {
        let cfg = PolicyConfig {
            max_daily_ops: 3,
            ..PolicyConfig::default()
        };
        let mut i = input(10);
        i.daily_ops = 2;
        assert!(evaluate(&cfg, &i).allowed);
        i.daily_ops = 3;
        assert_eq!(evaluate(&cfg, &i).reason, PolicyReason::DailyLimitReached);
        assert_eq!(
            require_policy(&cfg, &i),
            Err(Error::WithdrawalLimitExceeded)
        );
    }

    #[test]
    fn invalid_config_fails_closed() {
        let bad = PolicyConfig {
            min_amount: 0,
            ..PolicyConfig::default()
        };
        assert_eq!(
            evaluate(&bad, &input(10)).reason,
            PolicyReason::InvalidConfig
        );
        assert_eq!(require_policy(&bad, &input(10)), Err(Error::ConfigInvalid));

        let inverted = PolicyConfig {
            min_amount: 100,
            max_amount: 10,
            ..PolicyConfig::default()
        };
        assert_eq!(validate_policy(&inverted), Err(Error::ConfigInvalid));
    }
}
