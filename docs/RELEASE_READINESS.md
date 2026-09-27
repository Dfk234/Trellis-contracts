# Release Readiness (Issue #128)

Critical Trellis changes — anything that touches an on-chain interface,
storage layout, or emitted event — must pass a documented readiness
checklist before merge. This repo already gates most of that mechanically in
CI; this document is where it's all written down in one place, plus the one
enforcement CI was missing: nothing was checking that the checklist *itself*
had actually been completed on the PR.

## The checklist

Defined once, in [`.github/PULL_REQUEST_TEMPLATE.md`](../.github/PULL_REQUEST_TEMPLATE.md),
and automatically applied to every PR:

| Checklist item | Automated by |
|---|---|
| `cargo build --release` succeeds | `ci.yml` job `build` |
| Tests added/updated, `cargo test --workspace` passes | `ci.yml` job `test` |
| `cargo fmt --all -- --check` is clean | `ci.yml` job `format` |
| `cargo clippy --workspace -- -D warnings` is clean | `ci.yml` job `clippy` |
| `./scripts/security/run-audit.sh --skip-wasm` passes | `ci.yml` job `audit` |
| Docs updated where behaviour changed | reviewer judgment (not automatable) |
| No emitted event topic/payload changed, or migration note added | reviewer judgment + `changelog/entries.json` (see below) |
| No storage layout change, or migration described | reviewer judgment + `docs/MIGRATION_SAFETY.md` for the actual migration |

Two more CI jobs feed into readiness without being on the checklist directly:
`coverage` (floor: 70%) and `changelog` (validates
[`changelog/entries.json`](../changelog/entries.json) against its schema —
see [`docs/CHANGELOG_SCHEMA.md`](./CHANGELOG_SCHEMA.md)). A breaking change
without a `changelog` entry fails the schema check (missing `migration_notes`
would too, since the schema requires it non-empty for `impact: "breaking"`).

## What's now automated on top of that

`.github/workflows/release-readiness.yml` runs
[`scripts/check-pr-checklist.cjs`](../scripts/check-pr-checklist.cjs) against
the PR body on every open/edit/sync. It enforces:

- Every box under `## Checklist` and `## On-chain Interface` is checked.
- `## Type of Change` has at least one option selected.

This is the piece the per-job CI checks above cannot cover: a contributor
who deletes the checklist section, or leaves every box unchecked while CI
otherwise passes, is caught here instead of at review.

Run it locally against a draft PR body before opening:

```bash
node scripts/check-pr-checklist.cjs --body-file my-pr-body.md
```

## Emergency fixes and exceptions

An incident response (see [`docs/RUNBOOK.md`](./RUNBOOK.md)) sometimes needs
a fix merged before every box can be honestly checked — e.g. a pause
transaction that can't wait for a full audit re-run. For that case, add to
the PR body:

```markdown
<!-- release-readiness-exception: <why this can't wait, and who signed off> -->
```

`check-pr-checklist.cjs` then reports the unchecked items as an **acknowledged
exception** (logged, not silent) instead of failing the job. The reason is
mandatory and is not optional decoration — a bare
`<!-- release-readiness-exception: -->` with nothing after the colon does not
match the marker and the checklist is still enforced. Post-incident, open a
follow-up PR that actually completes the deferred items and reference the
original PR and incident channel in it.

## Validation

```bash
node scripts/check-pr-checklist.cjs --body-file <file>
node scripts/validate-changelog.cjs
```
