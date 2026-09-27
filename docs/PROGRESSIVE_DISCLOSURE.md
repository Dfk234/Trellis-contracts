# Progressive Disclosure of Transaction Details (Issue #125)

Users need access to advanced transaction metadata (routing, raw storage
keys, internal ids, timing) without that metadata overwhelming the common
case, and without a critical risk (an imminent expiry, a paused contract, a
below-floor price) ever ending up visible only in the part a caller has to
opt into.

The framework lives in [`shared::disclosure`](../shared/src/disclosure.rs).

## Why a builder instead of a convention

A naive approach — "advanced fields go in one list, everything else in
another, don't put warnings in the advanced list" — relies on every call
site remembering the rule. `TransactionDetailBuilder::push` enforces it
structurally instead: a field tagged [`DetailSeverity::Critical`] is placed
in the always-visible `summary` regardless of whether the caller asked for it
to be `advanced`. There is no code path that produces a `TransactionDetail`
with a critical field reachable only through the expanded view.

## Types

```rust
DetailSeverity = Info | Warning | Critical

DetailField {
    label, value,
    severity,
    requested_advanced,  // what the caller asked for — kept even when overridden
}

TransactionDetail {
    summary,   // always visible: non-advanced fields + every Critical field
    advanced,  // opt-in only: never contains a Critical field
}
```

## Building a detail view

```rust
let detail = TransactionDetailBuilder::new(&env)
    .push(symbol_short!("amount"), amount_str, DetailSeverity::Info, false)
    .push(symbol_short!("expiry"), expiry_str, DetailSeverity::Critical, true) // overridden into summary
    .push(symbol_short!("route"), route_str, DetailSeverity::Info, true)
    .finish();

detail.summary   // shown by default (collapsed state)
detail.advanced  // shown once the caller expands
detail.expanded() // everything — the read path is not gated behind expanding
```

`expanded()` exists so "advanced details are accessible before submission" is
a read a caller can make at any time, independent of whatever
collapsed/expanded UI state they're currently in.

## Verifying the invariant on a hand-built value

`TransactionDetail::critical_fields_are_never_advanced_only()` re-checks the
invariant for a `TransactionDetail` that didn't go through the builder (a
deserialized payload, a value built in a test fixture). It is what
`test_disclosure.rs`'s violation test uses to prove the check can actually
say `false`.

## Adopting this in a contract's preview/simulation read

A contract that exposes a preview before a state-changing call (a loan
request quote, a treasury withdrawal preview, an escrow settlement estimate)
should build its response as a `TransactionDetail`: the amount, counterparty,
and any risk (insufficient collateral, stale price, approaching deadline) as
`Critical` fields in `summary`; routing/internal detail as `Info`/`Warning`
fields requested as `advanced`. See `shared/src/disclosure.rs`'s module-level
doc example for the exact call shape.

## Validation

```bash
cargo test -p shared disclosure
```
