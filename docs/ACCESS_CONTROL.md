# Role-based access control

Trellis Contracts has no HTTP API, server-route layer, or client UI in this
repository. Soroban contract entrypoints are the authoritative authorization
boundary. A future API must reject unauthorized mutations by applying the same
server-side policy; hiding a UI control is never an authorization check.

## Roles and capabilities

`shared::auth::Permission` is the central capability vocabulary.
`role_for_permission`, `has_permission`, and `require_permission` map each
capability to the required on-chain role; `require_permission` also verifies
the caller's Soroban authorization.

| Actor / role | Capabilities | Enforcement |
|---|---|---|
| End user (`EndUser`) | Use only their own user-scoped resources | Entry point checks the resource owner and requires that address to authorize |
| Maintainer (`Admin`) | Change configuration, manage roles, read maintainer audit records | Admin role plus caller authorization; legacy admin-only entrypoints also verify the stored admin address |
| Treasury manager (`TreasuryManager`) | Deposit and withdraw treasury funds | `TreasuryOperations` permission |
| Pauser (`Pauser`) | Pause or resume the aid contract | `PauseContracts` permission; Admin can grant or revoke this role |
| Referral manager (`ReferralManager`) | Change referral configuration | `ReferralConfiguration` permission; Admin can grant or revoke this role |
| Oracle signer (`OracleSigner`) | Submit oracle updates | Oracle's explicit registered-submitter allowlist and signature check |
| Upgrader (`Upgrader`) | Propose or execute contract upgrades | `UpgradeContracts` permission |
| Service actor (`ServiceActor`) | Call explicitly registered contract-to-contract service operations | `ServiceOperation` permission and Soroban contract authorization |

End users are authorized by ownership and signature, not by a global role that
would grant access to other users' records. A contract may use `EndUser` for an
additional allowlist policy, but must still check resource ownership.

The treasury's configured referral contract receives `ServiceActor` when set;
replacing it revokes the prior service role. Other contract-to-contract
integrations should follow the same explicit-registration pattern.

The aid contract grants `Pauser` access through `set_pauser`. The referral
contract grants `ReferralManager` access through `set_referral_manager`.
Oracle submitters remain controlled by the oracle's existing explicit
registration/deactivation API.

## Initialization and upgrades

Shared-admin contracts use `initialize_admin`, which requires the initial
administrator's signature, records the admin role, and rejects a second
initialization. Contracts with a separate role registry apply equivalent
one-time and signature checks in their initializer. New role variants are
appended to the existing enum so stored role discriminants remain stable.

After upgrading an existing treasury deployment, its administrator must call
`set_referral_contract` with the currently configured referral address once to
grant that address the new `ServiceActor` role. No user funds or contract data
are migrated.

## Validation

```bash
cargo test -p shared auth
cargo test -p treasury-contract
```

The shared auth tests cover every permission-to-role mapping and denial after
revocation. Treasury contract tests cover authorized service payouts and reject
an unregistered contract actor.
