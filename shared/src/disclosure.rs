//! Progressive disclosure of advanced transaction details (Issue #125).
//!
//! Contracts that expose a preview/simulation read before a member submits a
//! transaction (loan requests, treasury withdrawals, escrow settlement, …)
//! tend to either dump every field on the caller at once, overwhelming the
//! common case, or trim the response down to just the essentials and lose
//! detail a power user or an auditor actually needs. Neither hides risk on
//! purpose, but a naive "advanced" section is an easy place for a warning to
//! end up unintentionally: nothing stops later code from tagging a critical
//! warning as `advanced` and having it disappear from the collapsed view.
//!
//! [`TransactionDetail`] makes that impossible by construction rather than by
//! convention: [`TransactionDetailBuilder::push`] promotes any field with
//! [`DetailSeverity::Critical`] into the always-visible summary regardless of
//! how the caller classified it, so a critical warning can never end up
//! reachable only through the expanded view.
//!
//! ```ignore
//! use soroban_sdk::{symbol_short, Env, String};
//! use shared::disclosure::{DetailSeverity, TransactionDetailBuilder};
//!
//! # let env = Env::default();
//! let detail = TransactionDetailBuilder::new(&env)
//!     .push(symbol_short!("amount"), String::from_str(&env, "1,000 XLM"), DetailSeverity::Info, false)
//!     .push(symbol_short!("expiry"), String::from_str(&env, "expires in 2 ledgers"), DetailSeverity::Critical, true)
//!     .push(symbol_short!("route"), String::from_str(&env, "via escrow #42"), DetailSeverity::Info, true)
//!     .finish();
//!
//! // Collapsed (default) view: the amount, and the "expiry" warning even
//! // though the caller asked to file it under advanced.
//! assert_eq!(detail.summary.len(), 2);
//! // Expanded view: everything, summary included — advanced details are
//! // accessible before submission, not gated behind it.
//! assert_eq!(detail.expanded().len(), 3);
//! ```

use soroban_sdk::{contracttype, Env, String, Symbol, Vec};

/// How much attention a detail field needs. `Critical` fields are always
/// visible — see [`TransactionDetailBuilder::push`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DetailSeverity {
    /// Ordinary informational detail.
    Info,
    /// Worth noticing, but not something that blocks a reasonable decision.
    Warning,
    /// A risk the caller must see before submitting, regardless of which
    /// disclosure state they're in.
    Critical,
}

/// One line of transaction detail: a label, its rendered value, how much
/// attention it needs, and whether the caller filed it as advanced.
///
/// `requested_advanced` is kept alongside the field (rather than only acting
/// on it during construction) so a UI can still show *why* a `Critical` field
/// is in the summary despite being requested as advanced — "this warning is
/// always shown" is a better message than silently overriding the caller's
/// intent with no trace of it.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetailField {
    pub label: Symbol,
    pub value: String,
    pub severity: DetailSeverity,
    /// What the caller asked for. May differ from where the field actually
    /// landed — see [`TransactionDetail::summary`] vs [`TransactionDetail::advanced`].
    pub requested_advanced: bool,
}

/// A transaction's detail fields, split into what's shown by default
/// ([`summary`](Self::summary)) and what a caller opts into
/// ([`advanced`](Self::advanced)). Every `Critical` field is guaranteed to be
/// in `summary` — see [`TransactionDetailBuilder::push`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransactionDetail {
    /// Always visible: every non-advanced field, plus every `Critical` field
    /// regardless of how it was requested.
    pub summary: Vec<DetailField>,
    /// Only shown once the caller expands "advanced details". Never
    /// contains a `Critical` field.
    pub advanced: Vec<DetailField>,
}

impl TransactionDetail {
    /// The full expanded view: `summary` followed by `advanced`, with no
    /// duplicates — this is the "advanced details are accessible before
    /// submission" read path, independent of a collapsed/expanded UI state.
    pub fn expanded(&self) -> Vec<DetailField> {
        let mut all = self.summary.clone();
        for field in self.advanced.iter() {
            all.push_back(field);
        }
        all
    }

    /// `true` if every field a caller tagged `Critical` ended up in
    /// `summary`. Always `true` for a `TransactionDetail` built through
    /// [`TransactionDetailBuilder`] — this exists so a `TransactionDetail`
    /// assembled by hand (e.g. deserialized, or built for a test fixture) can
    /// still be checked against the same invariant.
    pub fn critical_fields_are_never_advanced_only(&self) -> bool {
        !self
            .advanced
            .iter()
            .any(|field| field.severity == DetailSeverity::Critical)
    }
}

/// Builds a [`TransactionDetail`], enforcing that a `Critical` field can
/// never end up reachable only through the advanced view.
pub struct TransactionDetailBuilder {
    summary: Vec<DetailField>,
    advanced: Vec<DetailField>,
}

impl TransactionDetailBuilder {
    pub fn new(env: &Env) -> Self {
        Self {
            summary: Vec::new(env),
            advanced: Vec::new(env),
        }
    }

    /// Adds a field. `requested_advanced` is honored for `Info`/`Warning`
    /// severities; a `Critical` field is always placed in `summary`,
    /// regardless of what's requested — that's the whole point of this type.
    pub fn push(
        mut self,
        label: Symbol,
        value: String,
        severity: DetailSeverity,
        requested_advanced: bool,
    ) -> Self {
        let is_critical = severity == DetailSeverity::Critical;
        let field = DetailField {
            label,
            value,
            severity,
            requested_advanced,
        };
        if requested_advanced && !is_critical {
            self.advanced.push_back(field);
        } else {
            self.summary.push_back(field);
        }
        self
    }

    pub fn finish(self) -> TransactionDetail {
        TransactionDetail {
            summary: self.summary,
            advanced: self.advanced,
        }
    }
}
