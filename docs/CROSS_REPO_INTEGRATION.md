# Cross-Repository Typed Integration Contract

This document specifies the canonical typed contract and protocol for integrating external Trellis repositories (`Trellis-API`, `@trellis/sdk`, and frontend/indexer consumers) with the `Trellis-contracts` Soroban smart contracts.

---

## 1. Architecture & Design Principles

To ensure end-to-end type safety, deterministic communication, and error predictability across repositories, Trellis uses:
1. **Canonical Typed Contracts**: Core domain entities and operation arguments are defined centrally in `shared::client`.
2. **Standardized Status & Receipt Envelopes**: All operations return structured status codes and audit receipts (`ClientReceipt<T>`).
3. **Domain-Taxonomy Error Envelope**: Contract errors are decoded into client-friendly, retry-aware envelopes (`ClientErrorResponse`).
4. **Cryptographic Schema Fingerprinting**: A SHA-256 fingerprint of the client schema is computed on-chain to detect breaking schema changes across repo versions automatically.

---

## 2. Core Modules & Integration Types

### 2.1 Aid Disbursement Module
- **Request**: `CreateAidRequest { donor: Address, recipient: Address, amount: i128, expiry_ledger: u32 }`
- **Response**: `AidSummaryResponse { aid_id: u64, donor: Address, recipient: Address, token: Address, amount: i128, expiry_ledger: u32, status: OperationStatus, schema_version: u32 }`
- **Status Lifecycle**: `Pending` (0) → `Settled` (2) / `Refunded` (3)

### 2.2 Payment & Escrow Module
- **Request**: `CreateEscrowRequest { depositor: Address, beneficiary: Address, token: Address, amount: i128, expiry_ledger: u32 }`
- **Response**: `EscrowSummaryResponse { escrow_id: u64, depositor: Address, beneficiary: Address, token: Address, amount: i128, fee_amount: i128, expiry_ledger: u32, status: OperationStatus }`
- **Status Lifecycle**: `Active` (1) → `Completed` (2) / `Refunded` (3)

### 2.3 Multi-Sig Governance Module
- **Request**: `CreateProposalRequest { title: Symbol, target: Address, action_type: u32, parameter_key: u32, parameter_value: i128 }`
- **Response**: `ProposalSummaryResponse { proposal_id: u64, proposer: Address, status: OperationStatus, approval_count: u32, threshold: u32, created_at: u64, expires_at: u64 }`
- **Status Lifecycle**: `Pending` (0) → `Completed` (2) / `Cancelled` (4) / `Expired` (5)

### 2.4 NFT Marketplace Module
- **Request**: `CreateListingRequest { seller: Address, collection: Address, token_id: u64, price: i128, currency: Address }`
- **Response**: `ListingSummaryResponse { listing_id: u64, seller: Address, collection: Address, token_id: u64, price: i128, currency: Address, status: OperationStatus }`

### 2.5 Rebalancer Module
- **Request**: `RebalanceRequest { asset_in: Symbol, asset_out: Symbol, amount: u128, max_slippage_bps: u32 }`
- **Response**: `RebalanceSummaryResponse { status: OperationStatus, executed_amount: u128, fee_paid: u128, actual_slippage_bps: u32 }`

---

## 3. Universal Receipt & Error Envelopes

### 3.1 Client Execution Receipt
Every state-changing transaction produces a standardized receipt:
```rust
pub struct ClientReceipt<T> {
    pub operation_id: u64,
    pub domain: Symbol,
    pub status: OperationStatus,
    pub tx_hash: Option<BytesN<32>>,
    pub ledger: u32,
    pub timestamp: u64,
    pub data: T,
}
```

### 3.2 Client Error Envelope
Errors are normalized into actionable diagnostics with retry guidance:
```rust
pub struct ClientErrorResponse {
    pub domain: Symbol,
    pub code: u32,
    pub category: Symbol,
    pub retryable: bool,
    pub message: Symbol,
    pub recovery: Symbol,
    pub correlation_id: BytesN<32>,
}
```

---

## 4. Cross-Repo Compatibility & Upgrade Workflow

1. **Schema Verification**:
   - Call `client_schema_fingerprint()` to obtain the 32-byte checksum.
   - Client SDKs compare this with their embedded schema hash during initialization.
2. **Non-Breaking Additions**:
   - New optional fields or additive methods increment minor SDK versions without bumping `CLIENT_SCHEMA_VERSION`.
3. **Breaking Modifications**:
   - Any structural mutation to serialized types requires bumping `CLIENT_SCHEMA_VERSION` and publishing migration adapters in `shared::compat`.
