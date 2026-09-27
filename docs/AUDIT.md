# Maintainer audit trail

Sensitive treasury mutations are recorded in the contract's append-only
maintainer audit store in the same Soroban invocation as the state change. If
the audit write fails, the invocation fails rather than committing an
unaudited mutation.

## Covered treasury actions

| Action | Actor | Recorded context |
|---|---|---|
| One-time initialization | Initial admin | Initial withdrawal limit |
| Treasury-manager grant and revocation | Admin | Role state before and after |
| Withdrawal-limit change | Admin | Previous and new limit |
| Deposit and withdrawal | Treasury manager | Token, category, and balance before and after |
| Emergency withdrawal | Admin | Token, reserve category, and balance before and after |
| Referral-contract change | Admin | Whether a referral contract was configured |
| Referral reward payout | Registered referral contract | Rewards balance before and after |
| Quota configuration and reset | Admin | Stable action and reason codes |

The audit entry also includes the treasury scope, a stable action label, a
stable reason code, the ledger sequence, and ledger timestamp. Audit context
does not include free-form input, secrets, token destination addresses, or
recipient details. The affected token or administrative subject is retained
only where needed to identify the resource being changed. Existing ledger
events remain available for transaction details and indexing.

Every new entry is also emitted as a structured `("timeline", "audit_v2")`
ledger event, preserving the full record for off-chain export and long-term
review. The on-contract query reads the newest active persistent entries;
Soroban storage TTLs still apply to those queryable copies.

## Reading entries

The treasury contract exposes `audit_trail(maintainer, limit)` for structured
action records. The caller must authenticate and hold the shared `Admin` role.
Results are returned newest first; a limit of zero selects the default page
size and requests above the maximum are rejected. The legacy
`shared::timeline::audit_trail` accessor remains available for the original
audit-record schema. Both stores use separate keys and neither can be returned
from the participant-facing timeline.

The shared timeline API can also be used by other contract domains:
`record_action_audit_event` writes an authenticated action with scope, stable
action/reason symbols, affected resource and attribute when applicable, and
optional non-sensitive numeric before/after values; `action_audit_trail`
provides the maintainer-only query. New sensitive mutation paths should write
only after authorization and validation succeed, and should add tests for actor
attribution, event shape, and maintainer-only access.

## Validation

```bash
cargo test -p shared -p treasury-contract
```
