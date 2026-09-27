//! Privacy-preserving analytics aggregation (Issue #59).
//!
//! Maintainers need usage and reliability insight without ever exporting raw
//! user data, secrets, or sensitive payload content. This module enforces that
//! boundary structurally:
//!
//! - Observations may only be keyed by an allow-listed **safe dimension**
//!   (region bucket, role, schema version, …). Sensitive dimensions
//!   (`wallet`, `address`, `email`, `secret`, `payload`, …) are rejected
//!   outright with [`Error::InvalidArgument`].
//! - Only counts and sums leave the aggregator. Individual values are never
//!   stored or returned.
//! - Buckets smaller than [`PrivacyConfig::min_bucket_size`] are **suppressed**
//!   (k-anonymity) and reported only as an aggregate suppression count, so a
//!   single user cannot be re-identified from a bucket of one.
//! - Retained reports carry a metric version and a retention window so
//!   consumers know when to prune (see `docs/CONFIGURATION.md`).
//!
//! ## Metric definitions
//!
//! | Field | Definition |
//! |---|---|
//! | `count` | Number of observations in the bucket that passed validation. |
//! | `sum` | Arithmetic sum of the numeric measure for the bucket. |
//! | `suppressed` | Buckets dropped for falling below `min_bucket_size`. |
//! | `total_observations` | Observations considered, including suppressed ones. |

use soroban_sdk::{contracttype, symbol_short, Env, Symbol, Vec};

use crate::errors::Error;

/// Version stamped on every emitted report.
pub const ANALYTICS_METRIC_VERSION: u32 = 1;

/// Configuration controlling aggregation and privacy thresholds.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivacyConfig {
    /// Minimum observations required before a bucket is exported (k-anonymity).
    pub min_bucket_size: u32,
    /// Maximum distinct buckets a single report may contain.
    pub max_buckets: u32,
    /// How long (in ledgers) a report may be retained on-chain.
    pub retention_ledgers: u32,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            min_bucket_size: 5,
            max_buckets: 64,
            retention_ledgers: 17_280, // ~1 day at 5 s/ledger
        }
    }
}

/// A single observation. The dimension must be a safe, non-identifying key.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawObservation {
    pub dimension: Symbol,
    pub amount: i128,
}

/// An exported, anonymized bucket.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AggregateBucket {
    pub dimension: Symbol,
    pub count: u32,
    pub sum: i128,
}

/// The only shape analytics may leave the contract in.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsReport {
    pub metric_version: u32,
    pub buckets: Vec<AggregateBucket>,
    pub suppressed: u32,
    pub total_observations: u32,
}

/// Dimensions that may be used for aggregation.
pub fn is_safe_dimension(dimension: &Symbol) -> bool {
    // Allow-list: only these (and only these) may carry a report.
    dimension == &symbol_short!("region")
        || dimension == &symbol_short!("role")
        || dimension == &symbol_short!("tier")
        || dimension == &symbol_short!("version")
        || dimension == &symbol_short!("network")
}

/// Dimensions carrying identifying or secret material.
pub fn is_sensitive_dimension(dimension: &Symbol) -> bool {
    !is_safe_dimension(dimension)
}

/// Aggregate observations into a privacy-safe report.
///
/// Buckets are emitted in first-appearance order so the output is deterministic
/// for a given input. Buckets below `min_bucket_size` are suppressed and only
/// counted. Violations map to stable, user-safe errors:
///
/// - sensitive dimension -> [`Error::InvalidArgument`]
/// - negative amount -> [`Error::InvalidAmount`]
/// - too many distinct buckets -> [`Error::QuotaExceeded`]
/// - invalid config -> [`Error::ConfigInvalid`]
pub fn aggregate_events(
    env: &Env,
    observations: &Vec<RawObservation>,
    config: &PrivacyConfig,
) -> Result<AnalyticsReport, Error> {
    if config.min_bucket_size == 0 || config.max_buckets == 0 || config.retention_ledgers == 0 {
        return Err(Error::ConfigInvalid);
    }

    let mut keys: Vec<Symbol> = Vec::new(env);
    let mut counts: Vec<u32> = Vec::new(env);
    let mut sums: Vec<i128> = Vec::new(env);

    for i in 0..observations.len() {
        let obs = observations.get(i).unwrap();
        if is_sensitive_dimension(&obs.dimension) {
            return Err(Error::InvalidArgument);
        }
        if obs.amount < 0 {
            return Err(Error::InvalidAmount);
        }

        let mut found: Option<u32> = None;
        for j in 0..keys.len() {
            if keys.get(j).unwrap() == obs.dimension {
                found = Some(j);
                break;
            }
        }

        match found {
            Some(j) => {
                counts.set(j, counts.get(j).unwrap().saturating_add(1));
                sums.set(j, sums.get(j).unwrap().saturating_add(obs.amount));
            }
            None => {
                if keys.len() >= config.max_buckets {
                    return Err(Error::QuotaExceeded);
                }
                keys.push_back(obs.dimension.clone());
                counts.push_back(1);
                sums.push_back(obs.amount);
            }
        }
    }

    let mut buckets: Vec<AggregateBucket> = Vec::new(env);
    let mut suppressed: u32 = 0;
    for j in 0..keys.len() {
        let count = counts.get(j).unwrap();
        if count < config.min_bucket_size {
            suppressed = suppressed.saturating_add(1);
            continue;
        }
        buckets.push_back(AggregateBucket {
            dimension: keys.get(j).unwrap(),
            count,
            sum: sums.get(j).unwrap(),
        });
    }

    Ok(AnalyticsReport {
        metric_version: ANALYTICS_METRIC_VERSION,
        buckets,
        suppressed,
        total_observations: observations.len(),
    })
}

/// Returns `true` when a report contains no exporting bucket and therefore
/// reveals nothing beyond the suppression count.
pub fn is_fully_suppressed(report: &AnalyticsReport) -> bool {
    report.buckets.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(dimension: Symbol, amount: i128) -> RawObservation {
        RawObservation { dimension, amount }
    }

    fn feed(env: &Env, items: &[(&str, i128)]) -> Vec<RawObservation> {
        let mut out: Vec<RawObservation> = Vec::new(env);
        for (k, v) in items.iter() {
            out.push_back(obs(
                match *k {
                    "region" => symbol_short!("region"),
                    "role" => symbol_short!("role"),
                    "tier" => symbol_short!("tier"),
                    "wallet" => symbol_short!("wallet"),
                    "email" => symbol_short!("email"),
                    _ => symbol_short!("version"),
                },
                *v,
            ));
        }
        out
    }

    #[test]
    fn buckets_below_k_are_suppressed() {
        let env = Env::default();
        let data = feed(&env, &[("region", 10), ("region", 10), ("role", 5)]);
        let report = aggregate_events(&env, &data, &PrivacyConfig::default()).unwrap();
        assert!(report.buckets.is_empty());
        assert_eq!(report.suppressed, 2);
        assert!(is_fully_suppressed(&report));
        assert_eq!(report.total_observations, 3);
    }

    #[test]
    fn exported_buckets_only_contain_counts_and_sums() {
        let env = Env::default();
        let mut data: Vec<RawObservation> = Vec::new(&env);
        for _ in 0..6 {
            data.push_back(obs(symbol_short!("region"), 7));
        }
        data.push_back(obs(symbol_short!("role"), 1));
        let report = aggregate_events(&env, &data, &PrivacyConfig::default()).unwrap();
        assert_eq!(report.buckets.len(), 1);
        let b = report.buckets.get(0).unwrap();
        assert_eq!(b.dimension, symbol_short!("region"));
        assert_eq!(b.count, 6);
        assert_eq!(b.sum, 42);
        assert_eq!(report.suppressed, 1);
    }

    #[test]
    fn sensitive_dimensions_are_rejected() {
        let env = Env::default();
        let data = feed(&env, &[("wallet", 1)]);
        assert_eq!(
            aggregate_events(&env, &data, &PrivacyConfig::default()),
            Err(Error::InvalidArgument)
        );
    }

    #[test]
    fn bucket_cap_is_enforced() {
        let env = Env::default();
        let mut data: Vec<RawObservation> = Vec::new(&env);
        data.push_back(obs(symbol_short!("region"), 1));
        data.push_back(obs(symbol_short!("role"), 1));
        let cfg = PrivacyConfig {
            max_buckets: 1,
            ..PrivacyConfig::default()
        };
        assert_eq!(
            aggregate_events(&env, &data, &cfg),
            Err(Error::QuotaExceeded)
        );
    }

    #[test]
    fn invalid_config_and_amounts_are_rejected() {
        let env = Env::default();
        let data = feed(&env, &[("region", 1)]);
        let bad = PrivacyConfig {
            min_bucket_size: 0,
            ..PrivacyConfig::default()
        };
        assert_eq!(
            aggregate_events(&env, &data, &bad),
            Err(Error::ConfigInvalid)
        );

        let negative = feed(&env, &[("region", -1)]);
        assert_eq!(
            aggregate_events(&env, &negative, &PrivacyConfig::default()),
            Err(Error::InvalidAmount)
        );
    }
}
