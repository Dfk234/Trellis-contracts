#!/usr/bin/env bash
#
# Local keeper for the shared background worker framework (Issue #35).
#
# Repeatedly invokes the permissionless `run_due_job` entry point of a contract
# that forwards to `shared::jobs::run_due_job`. Holds no privileged key: any
# account can crank the queue, so run as many of these as you like.
#
# Usage:
#   WORKER_CONTRACT_ID=<id> WORKER_SOURCE=<key> ./scripts/run-worker.sh
#
# Environment:
#   WORKER_CONTRACT_ID    Contract exposing `run_due_job` (required)
#   WORKER_SOURCE         Stellar CLI key name to sign with (default: worker)
#   WORKER_ADDRESS        Address recorded on the job (default: address of WORKER_SOURCE)
#   STELLAR_NETWORK       Network name/passphrase for the CLI (default: local)
#   WORKER_INTERVAL       Seconds between cranks (default: 5)
#   WORKER_MAX_CRANKS     Stop after N cranks, 0 = run forever (default: 0)
set -euo pipefail

: "${WORKER_CONTRACT_ID:?set WORKER_CONTRACT_ID to the contract exposing run_due_job}"
WORKER_SOURCE="${WORKER_SOURCE:-worker}"
STELLAR_NETWORK="${STELLAR_NETWORK:-local}"
WORKER_INTERVAL="${WORKER_INTERVAL:-5}"
WORKER_MAX_CRANKS="${WORKER_MAX_CRANKS:-0}"

if [[ -z "${WORKER_ADDRESS:-}" ]]; then
  WORKER_ADDRESS="$(stellar keys address "$WORKER_SOURCE")"
fi

echo "worker: contract=${WORKER_CONTRACT_ID} source=${WORKER_SOURCE} address=${WORKER_ADDRESS}"
echo "worker: network=${STELLAR_NETWORK} interval=${WORKER_INTERVAL}s max_cranks=${WORKER_MAX_CRANKS}"

cleanup() {
  echo
  echo "worker: stopped"
}
trap cleanup EXIT INT TERM

cranks=0
while true; do
  outcome="$(
    stellar contract invoke \
      --id "$WORKER_CONTRACT_ID" \
      --source "$WORKER_SOURCE" \
      --network "$STELLAR_NETWORK" \
      -- run_due_job --worker "$WORKER_ADDRESS" 2>&1
  )" || {
    # A failed crank is normal: the job it picked may have failed and been
    # rescheduled, or the worker may be paused. Log and keep polling.
    echo "worker: crank error: ${outcome//$'\n'/ }"
    outcome=""
  }

  if [[ -n "$outcome" ]]; then
    echo "worker: crank $((cranks + 1)) -> ${outcome//$'\n'/ }"
  fi

  cranks=$((cranks + 1))
  if [[ "$WORKER_MAX_CRANKS" != "0" && "$cranks" -ge "$WORKER_MAX_CRANKS" ]]; then
    echo "worker: reached WORKER_MAX_CRANKS=${WORKER_MAX_CRANKS}"
    break
  fi

  sleep "$WORKER_INTERVAL"
done
