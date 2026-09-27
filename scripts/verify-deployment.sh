#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Trellis cross-contract deployment verification smoke test (Issue #101)
#
# Validates that a testnet/mainnet deployment is wired together correctly by
# reading the recorded contract addresses and querying each contract's getters
# on-chain. Catches the failure modes where `scripts/deploy.sh` succeeded but
# `scripts/initialize.sh` (or a manual wiring step) left the system broken:
#
#   * AidContract::get_treasury           == deployed TreasuryContract
#   * TreasuryContract::referral_contract == deployed ReferralContract
#   * ReferralContract::get_treasury      == deployed TreasuryContract
#   * RegistryContract::get_contract      resolves every deployed contract to
#                                         the exact address recorded for it
#   * (optional) a zero-value, read-only simulation call per core contract to
#     confirm the contract is reachable and the configured source may invoke it
#
# Usage:
#   ./scripts/verify-deployment.sh [testnet|mainnet] [options]
#
# Options:
#   --network <net>          testnet | mainnet            (default: testnet)
#   --deployments <file>     Deployment JSON from deploy.sh
#                            (default: .deployment-log-<net>.json)
#   --deployments-md <file>  DEPLOYMENTS.md fallback      (default: DEPLOYMENTS.md)
#   --source <identity>      soroban source identity      (default: admin)
#   --rpc-url <url>          Override the RPC endpoint for the network
#   --registry-name k=sym    Registry Symbol to look up for contract key `k`
#                            (repeatable; overrides the default map below)
#   --skip-simulation        Skip the zero-value invocation simulation
#   --json                   Emit a machine-readable summary instead of a report
#   --help                   Show this help
#
# Exit codes:
#   0  every check passed
#   1  one or more checks failed (missing/mismatched address, invoke error)
#   2  the smoke test could not run (no deployment data, missing tooling)
#
# Guarantees:
#   * Read-only: never signs or submits a state-changing transaction and never
#     moves funds. The simulation call is sent with `--send=no` when the
#     installed soroban CLI supports it.
#   * Every failure prints an actionable remediation line.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "${SCRIPT_DIR}")"

NETWORK="testnet"
DEPLOYMENTS_FILE=""
DEPLOYMENTS_MD="${REPO_ROOT}/DEPLOYMENTS.md"
SOURCE="admin"
RPC_URL=""
SKIP_SIM=0
JSON_OUT=0

# key|contract_dir|wasm_name|readable_name|registry_symbol|required
CONTRACT_TABLE='aid|aid-contract|aid_contract|Aid Contract|aid|required
treasury|treasury-contract|treasury_contract|Treasury Contract|treasury|required
referral|referral-contract|referral_contract|Referral Contract|referral|required
registry|registry-contract|registry_contract|Registry Contract|registry|required
governance|governance-contract|governance_contract|Governance Contract|governance|optional
oracle|oracle-contract|oracle_contract|Oracle Contract|oracle|optional
payments|payments-contract|payments_contract|Payments Contract|payments|optional
nft|nft-marketplace|nft_marketplace|NFT Marketplace|nft|optional
access|access-control|access_control|Access Control|access|optional
upgradeability|upgradeability|upgradeability|Upgradeability|upgradeability|optional
rebalancer|rebalancer-contract|rebalancer_contract|Rebalancer Contract|rebalancer|optional'

# key|read-only entry point used for the zero-value invocation simulation
# (every function below takes no arguments and mutates no state)
SIM_TABLE='aid|is_initialized
treasury|referral_contract
referral|get_treasury
registry|list_names'

# ---------------------------------------------------------------------------
# Helpers (defined before any executable logic)
# ---------------------------------------------------------------------------
usage() {
  cat <<'EOF'
Trellis cross-contract deployment verification smoke test.

Usage:
  ./scripts/verify-deployment.sh [testnet|mainnet] [options]

Options:
  --network <net>          testnet | mainnet            (default: testnet)
  --deployments <file>     Deployment JSON from deploy.sh
                           (default: .deployment-log-<net>.json)
  --deployments-md <file>  DEPLOYMENTS.md fallback      (default: DEPLOYMENTS.md)
  --source <identity>      soroban source identity      (default: admin)
  --rpc-url <url>          Override the RPC endpoint for the network
  --registry-name k=sym    Registry Symbol to look up for contract key `k`
                           (repeatable; overrides the default map)
  --skip-simulation        Skip the zero-value invocation simulation
  --json                   Emit a machine-readable summary instead of a report
  --help                   Show this help

Checks:
  * AidContract::get_treasury           == deployed TreasuryContract
  * TreasuryContract::referral_contract == deployed ReferralContract
  * ReferralContract::get_treasury      == deployed TreasuryContract
  * RegistryContract::get_contract      resolves every deployed contract
  * read-only invocation simulation for each core contract

Exit codes: 0 = all passed, 1 = failures, 2 = could not run.
EOF
  exit 0
}

die() {
  printf 'VERIFY ERROR: %s\n' "$1" >&2
  if [ -n "${2:-}" ]; then printf '  Fix: %s\n' "$2" >&2; fi
  exit 2
}

one_line() { printf '%s' "$1" | tr '\n' ' ' | sed 's/  */ /g' | cut -c1-200; }

extract_contract_id() {
  printf '%s' "$1" | grep -oE 'C[A-Z0-9]{55}' | head -1 || true
}

addr_get() { grep -E "^$1=" "${ADDR_FILE}" 2>/dev/null | tail -1 | cut -d= -f2-; }

key_for_dir() {
  while IFS='|' read -r k d w r s req; do
    if [ "$d" = "$1" ]; then printf '%s' "$k"; return 0; fi
  done <<< "${CONTRACT_TABLE}"
  printf ''
}

key_for_wasm() {
  while IFS='|' read -r k d w r s req; do
    if [ "$w" = "$1" ]; then printf '%s' "$k"; return 0; fi
  done <<< "${CONTRACT_TABLE}"
  printf ''
}

key_for_readable() {
  while IFS='|' read -r k d w r s req; do
    if [ "$r" = "$1" ]; then printf '%s' "$k"; return 0; fi
  done <<< "${CONTRACT_TABLE}"
  printf ''
}

readable_for() {
  while IFS='|' read -r k d w r s req; do
    if [ "$k" = "$1" ]; then printf '%s' "$r"; return 0; fi
  done <<< "${CONTRACT_TABLE}"
  printf '%s' "$1"
}

default_symbol_for() {
  while IFS='|' read -r k d w r s req; do
    if [ "$k" = "$1" ]; then printf '%s' "$s"; return 0; fi
  done <<< "${CONTRACT_TABLE}"
  printf ''
}

sym_for() {
  local override
  override="$(grep -E "^$1=" "${SYM_FILE}" 2>/dev/null | tail -1 || true)"
  if [ -n "${override}" ]; then printf '%s' "${override#*=}"; return 0; fi
  default_symbol_for "$1"
}

sim_fn_for() {
  local k fn
  while IFS='|' read -r k fn; do
    if [ "$k" = "$1" ]; then printf '%s' "$fn"; return 0; fi
  done <<< "${SIM_TABLE}"
  printf ''
}

record() { # record <name> <pass|fail> <detail>
  local name="$1" status="$2" detail="$3" label="FAIL"
  printf '%s|%s|%s\n' "${name}" "${status}" "${detail}" >> "${RESULTS_FILE}"
  if [ "${status}" = "pass" ]; then
    PASS=$((PASS + 1)); label="PASS"
  else
    FAIL=$((FAIL + 1))
  fi
  if [ "${JSON_OUT}" -eq 0 ]; then
    printf '  [%s] %s\n' "${label}" "${name}"
    if [ -n "${detail}" ]; then printf '        %s\n' "${detail}"; fi
  fi
}

say() { if [ "${JSON_OUT}" -eq 0 ]; then printf '%s\n' "$1"; fi; }

json_escape() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\t/ /g'; }

emit_json() {
  printf '{"network":"%s","source":"%s","pass":%d,"fail":%d,"checks":[' \
    "$(json_escape "${NETWORK}")" "$(json_escape "${SOURCE}")" "${PASS}" "${FAIL}"
  local first=1 name status detail
  while IFS='|' read -r name status detail; do
    [ -n "${name}" ] || continue
    if [ "${first}" -eq 0 ]; then printf ','; fi
    first=0
    printf '{"name":"%s","status":"%s","detail":"%s"}' \
      "$(json_escape "${name}")" "${status}" "$(json_escape "${detail}")"
  done < "${RESULTS_FILE}"
  printf ']}\n'
}

resolve_from_json() {
  local file="$1" rows dir wasm id key
  [ -f "${file}" ] || return 1
  command -v jq >/dev/null 2>&1 || die "jq is required to read ${file}" "install jq (apt install jq / brew install jq)"
  rows="$(jq -r '.deployments[]? | [.contract_dir, .wasm_name, .contract_id] | @tsv' "${file}" 2>/dev/null)" || return 1
  [ -n "${rows}" ] || return 1
  while IFS=$'\t' read -r dir wasm id; do
    [ -n "${id}" ] || continue
    id="$(extract_contract_id "${id}")"
    [ -n "${id}" ] || continue
    key="$(key_for_dir "${dir}")"
    [ -n "${key}" ] || key="$(key_for_wasm "${wasm}")"
    [ -n "${key}" ] || continue
    printf '%s=%s\n' "${key}" "${id}" >> "${ADDR_FILE}"
  done <<< "${rows}"
  return 0
}

resolve_from_md() {
  local file="$1" section rows name addr key
  [ -f "${file}" ] || return 1
  section="## $(awk -v n="${NETWORK}" 'BEGIN{print toupper(substr(n,1,1)) substr(n,2)}') Deployments"
  rows="$(awk -v sect="${section}" '
    $0 == sect { insec = 1; next }
    insec && /^## / { exit }
    insec && /^\|/ {
      line = $0
      sub(/^\|/, "", line); sub(/\|$/, "", line)
      n = split(line, cells, "|")
      name = cells[1]; addr = cells[2]
      gsub(/^[ \t]+|[ \t]+$/, "", name)
      gsub(/^[ \t]+|[ \t]+$/, "", addr)
      if (name == "" || name == "Contract" || name ~ /^-+$/) next
      if (addr ~ /^C[A-Z0-9]{55}$/) print name "\t" addr
    }
  ' "${file}")"
  [ -n "${rows}" ] || return 1
  while IFS=$'\t' read -r name addr; do
    key="$(key_for_readable "${name}")"
    [ -n "${key}" ] || continue
    printf '%s=%s\n' "${key}" "${addr}" >> "${ADDR_FILE}"
  done <<< "${rows}"
  return 0
}

SIM_MODE=0
SIM_FLAG=""

detect_sim_flag() {
  local help
  help="$(soroban contract invoke --help 2>&1 || true)"
  if printf '%s' "${help}" | grep -q -- '--send'; then
    printf '%s' '--send=no'
  elif printf '%s' "${help}" | grep -q -- 'simulate-only'; then
    printf '%s' '--simulate-only'
  else
    printf ''
  fi
}

invoke_raw() { # <contract_id> <fn> [args...]
  local id="$1"; shift
  if [ "${SIM_MODE}" -eq 1 ] && [ -n "${SIM_FLAG}" ]; then
    if [ -n "${RPC_URL}" ]; then
      soroban contract invoke --id "${id}" --network "${NETWORK}" --source "${SOURCE}" --rpc-url "${RPC_URL}" "${SIM_FLAG}" -- "$@"
    else
      soroban contract invoke --id "${id}" --network "${NETWORK}" --source "${SOURCE}" "${SIM_FLAG}" -- "$@"
    fi
  else
    if [ -n "${RPC_URL}" ]; then
      soroban contract invoke --id "${id}" --network "${NETWORK}" --source "${SOURCE}" --rpc-url "${RPC_URL}" -- "$@"
    else
      soroban contract invoke --id "${id}" --network "${NETWORK}" --source "${SOURCE}" -- "$@"
    fi
  fi
}

OUT=""
RC=0
try() { OUT="$(invoke_raw "$@" 2>&1)"; RC=$?; }

# ---------------------------------------------------------------------------
# Temp state + argument parsing
# ---------------------------------------------------------------------------
TMP_RUN="$(mktemp -d 2>/dev/null || mktemp -d -t trellis-verify)"
trap 'rm -rf "${TMP_RUN}"' EXIT

ADDR_FILE="${TMP_RUN}/addresses"
SYM_FILE="${TMP_RUN}/symbols"
RESULTS_FILE="${TMP_RUN}/results"
: > "${ADDR_FILE}"
: > "${SYM_FILE}"
: > "${RESULTS_FILE}"

PASS=0
FAIL=0

while [ $# -gt 0 ]; do
  case "$1" in
    --network) NETWORK="${2:-}"; shift 2 ;;
    --deployments) DEPLOYMENTS_FILE="${2:-}"; shift 2 ;;
    --deployments-md) DEPLOYMENTS_MD="${2:-}"; shift 2 ;;
    --source) SOURCE="${2:-}"; shift 2 ;;
    --rpc-url) RPC_URL="${2:-}"; shift 2 ;;
    --registry-name)
      [ -n "${2:-}" ] || die "--registry-name requires key=symbol"
      printf '%s\n' "$2" >> "${SYM_FILE}"
      shift 2
      ;;
    --skip-simulation) SKIP_SIM=1; shift ;;
    --json) JSON_OUT=1; shift ;;
    --help|-h) usage ;;
    testnet|mainnet) NETWORK="$1"; shift ;;
    *) die "unknown argument: $1" "run ./scripts/verify-deployment.sh --help" ;;
  esac
done

case "${NETWORK}" in
  testnet|mainnet) ;;
  *) die "unsupported network '${NETWORK}'" "pass testnet or mainnet" ;;
esac

# ---------------------------------------------------------------------------
# 0. Prerequisites
# ---------------------------------------------------------------------------
say "========================================"
say "Trellis deployment verification"
say "Network:  ${NETWORK}"
say "Source:   ${SOURCE}"
say "========================================"
say ""

command -v soroban >/dev/null 2>&1 || die "soroban CLI not found on PATH" "cargo install --locked soroban-cli"

# ---------------------------------------------------------------------------
# 1. Resolve recorded addresses
# ---------------------------------------------------------------------------
if [ -z "${DEPLOYMENTS_FILE}" ]; then
  DEPLOYMENTS_FILE="${REPO_ROOT}/.deployment-log-${NETWORK}.json"
fi

say "[1/5] Resolving deployed addresses..."
RESOLVED_FROM=""
if resolve_from_json "${DEPLOYMENTS_FILE}"; then
  RESOLVED_FROM="${DEPLOYMENTS_FILE}"
elif resolve_from_md "${DEPLOYMENTS_MD}"; then
  RESOLVED_FROM="${DEPLOYMENTS_MD} (markdown fallback)"
fi

if [ -z "${RESOLVED_FROM}" ]; then
  die "${NETWORK} contracts are not deployed yet (no addresses in ${DEPLOYMENTS_FILE} or ${DEPLOYMENTS_MD})" \
    "run ./scripts/deploy.sh ${NETWORK} then ./scripts/record-deployments.sh ${NETWORK}"
fi

say "  source of truth: ${RESOLVED_FROM}"
for key in aid treasury referral registry; do
  id="$(addr_get "${key}")"
  if [ -n "${id}" ]; then
    say "  - $(readable_for "${key}"): ${id}"
  else
    say "  - $(readable_for "${key}"): (missing)"
  fi
done
say ""

for key in aid treasury referral registry; do
  id="$(addr_get "${key}")"
  if [ -z "${id}" ]; then
    record "address:${key}" fail "$(readable_for "${key}") address missing from deployment data; run ./scripts/deploy.sh ${NETWORK} and ./scripts/record-deployments.sh ${NETWORK}"
  else
    record "address:${key}" pass "$(readable_for "${key}") -> ${id}"
  fi
done

if [ "${FAIL}" -gt 0 ]; then
  say "Cannot continue: required addresses are missing."
  [ "${JSON_OUT}" -eq 1 ] && emit_json
  exit 1
fi

# ---------------------------------------------------------------------------
# 2. Cross-contract linkage getters
# ---------------------------------------------------------------------------
say ""
say "[2/5] Cross-contract linkage..."

check_link() { # <check name> <contract key> <getter> <expected key>
  local name="$1" ckey="$2" getter="$3" expect_key="$4"
  local cid exp got
  cid="$(addr_get "${ckey}")"
  exp="$(addr_get "${expect_key}")"
  if [ -z "${exp}" ]; then
    record "${name}" fail "expected $(readable_for "${expect_key}") address is missing from deployment data"
    return
  fi
  try "${cid}" "${getter}"
  if [ "${RC}" -ne 0 ]; then
    record "${name}" fail "$(readable_for "${ckey}")::${getter}() invoke failed (rc=${RC}): $(one_line "${OUT}")"
    return
  fi
  got="$(extract_contract_id "${OUT}")"
  if [ -z "${got}" ]; then
    record "${name}" fail "$(readable_for "${ckey}")::${getter}() returned no address (unset?) -> $(one_line "${OUT}")"
    return
  fi
  if [ "${got}" = "${exp}" ]; then
    record "${name}" pass "$(readable_for "${ckey}")::${getter}() -> ${got}  (matches $(readable_for "${expect_key}"))"
  else
    record "${name}" fail "$(readable_for "${ckey}")::${getter}() -> ${got} but $(readable_for "${expect_key}") is ${exp}; re-run ./scripts/initialize.sh with the correct wiring"
  fi
}

check_link "link:aid.get_treasury==treasury" aid get_treasury treasury
check_link "link:treasury.referral_contract==referral" treasury referral_contract referral
check_link "link:referral.get_treasury==treasury" referral get_treasury treasury

# ---------------------------------------------------------------------------
# 3. Registry entries for every deployed contract
# ---------------------------------------------------------------------------
say ""
say "[3/5] Registry coverage..."

registry_id="$(addr_get registry)"
keys="$(cut -d= -f1 "${ADDR_FILE}" | sort -u)"

for key in ${keys}; do
  [ -n "${key}" ] || continue
  sym="$(sym_for "${key}")"
  exp="$(addr_get "${key}")"
  if [ -z "${sym}" ]; then
    record "registry:${key}" fail "no registry Symbol mapping for '${key}'; pass --registry-name ${key}=<symbol>"
    continue
  fi
  try "${registry_id}" get_contract --name "${sym}"
  if [ "${RC}" -ne 0 ]; then
    record "registry:${key}" fail "RegistryContract::get_contract(${sym}) not found -> register it: soroban contract invoke --id ${registry_id} --network ${NETWORK} --source ${SOURCE} -- set_contract --caller ${SOURCE} --name ${sym} --address ${exp} --version 1"
    continue
  fi
  got="$(extract_contract_id "${OUT}")"
  if [ -z "${got}" ]; then
    record "registry:${key}" fail "RegistryContract::get_contract(${sym}) returned no address -> $(one_line "${OUT}")"
    continue
  fi
  if [ "${got}" = "${exp}" ]; then
    record "registry:${key}" pass "RegistryContract::get_contract(${sym}) -> ${got}  (matches $(readable_for "${key}"))"
  else
    record "registry:${key}" fail "RegistryContract::get_contract(${sym}) -> ${got} but $(readable_for "${key}") is ${exp}; update the registry entry"
  fi
done

# ---------------------------------------------------------------------------
# 4. Zero-value, read-only invocation simulation
# ---------------------------------------------------------------------------
say ""
if [ "${SKIP_SIM}" -eq 1 ]; then
  say "[4/5] Invocation simulation skipped (--skip-simulation)."
else
  say "[4/5] Invocation simulation (read-only, no value)..."
  SIM_FLAG="$(detect_sim_flag)"
  if [ -z "${SIM_FLAG}" ]; then
    say "  note: this soroban CLI exposes no --send/simulate-only flag; invoking read-only getters directly."
  fi
  SIM_MODE=1
  for key in aid treasury referral registry; do
    fn="$(sim_fn_for "${key}")"
    [ -n "${fn}" ] || continue
    cid="$(addr_get "${key}")"
    [ -n "${cid}" ] || continue
    try "${cid}" "${fn}"
    if [ "${RC}" -eq 0 ]; then
      record "simulate:${key}::${fn}" pass "$(readable_for "${key}") accepted the simulated call (no value, no state change)"
    else
      record "simulate:${key}::${fn}" fail "$(readable_for "${key}")::${fn}() simulation failed (rc=${RC}): $(one_line "${OUT}")"
    fi
  done
  SIM_MODE=0
fi

# ---------------------------------------------------------------------------
# 5. Summary
# ---------------------------------------------------------------------------
say ""
if [ "${JSON_OUT}" -eq 1 ]; then
  emit_json
else
  say "========================================"
  say "Result: ${PASS} passed, ${FAIL} failed"
  if [ "${FAIL}" -eq 0 ]; then
    say "All cross-contract checks passed for ${NETWORK}."
  else
    say "Fix each FAIL line above, then re-run ./scripts/verify-deployment.sh ${NETWORK}."
  fi
  say "========================================"
fi

[ "${FAIL}" -eq 0 ]
