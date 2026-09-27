# Machine-Readable Changelog (Issue #126)

`CHANGELOG.md` is a human changelog (prose, [Keep a Changelog](https://keepachangelog.com/) format) —
good for a release note, but nothing can query it and nothing validates that
an entry actually says what an integrator needs. `changelog/entries.json` is
the machine-readable counterpart, scoped narrowly to **protocol-facing**
changes: anything that changes what an on-chain caller, an indexer, or a
client SDK observes — a storage layout change, a schema version bump, an
event topic/payload change, a newly-enforced validation rule.

It does **not** duplicate every `CHANGELOG.md` entry. Internal refactors, doc
fixes, and test-only additions stay in `CHANGELOG.md` alone; only changes an
integrator would need to react to (or at least know about) get a
`changelog/entries.json` entry.

## Schema

Defined formally in [`changelog/schema.json`](../changelog/schema.json)
(JSON Schema draft-07). Each entry:

| Field | Required | Meaning |
|---|---|---|
| `version` | yes | The repository version this ships in, or `"unreleased"`. |
| `date` | yes | Merge date, `YYYY-MM-DD`. |
| `impact` | yes | `breaking` \| `compatible` \| `internal` — see below. |
| `summary` | yes | One sentence, from the caller/indexer's point of view. |
| `migration_notes` | yes | What an integrator must do. **Non-empty when `impact` is `breaking`** — the schema enforces this, it is not just a convention. |
| `issue` | yes (nullable) | The GitHub issue this closes. |
| `pr` | yes (nullable) | The GitHub PR that merged this. |

### `impact` values

- **`breaking`** — an existing caller or indexer must change something to
  keep working. `migration_notes` must explain what.
- **`compatible`** — additive; existing callers are unaffected (a new
  optional field, a new opt-in module, a new entry point).
- **`internal`** — protocol-adjacent but not itself an observable behavior
  change (shared-library tooling nothing yet calls, test infrastructure).

## Validating

```bash
node scripts/validate-changelog.cjs
```

Exit codes: `0` valid, `1` a schema violation (details printed), `2` a file
is missing or malformed JSON (cannot even attempt validation). This runs as
its own job in CI (`.github/workflows/ci.yml`, job `changelog`) on every push
and PR, so an invalid or missing-`migration_notes` breaking entry fails the
build rather than reaching review.

## When contributors must add an entry

Add a `changelog/entries.json` entry in the same PR whenever the change:

- alters a storage key, layout, or `#[contracttype]` shape that's already
  shipped (see also `docs/COMPATIBILITY.md`'s versioned-record pattern —
  most schema changes should go through that instead of breaking readers
  outright);
- adds, removes, or changes the meaning of an emitted event topic or payload
  field;
- changes what a previously-accepted call now rejects, or vice versa;
- bumps a schema/record version constant.

A refactor with no observable difference, a doc update, or a new test file
does not need one — `CHANGELOG.md`'s `[Unreleased]` section is still the
right (and lower-ceremony) place for those.
