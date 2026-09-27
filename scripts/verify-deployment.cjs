#!/usr/bin/env node
/**
 * Trellis cross-contract deployment verification smoke test — cross-platform
 * Node.js runner (Issue #101). Windows equivalent of `scripts/verify-deployment.sh`.
 *
 * It reads the recorded contract addresses (deploy.sh output JSON, falling back
 * to DEPLOYMENTS.md), then queries each contract's read-only getters through the
 * `soroban` CLI and verifies:
 *
 *   * AidContract::get_treasury           == deployed TreasuryContract
 *   * TreasuryContract::referral_contract == deployed ReferralContract
 *   * ReferralContract::get_treasury      == deployed TreasuryContract
 *   * RegistryContract::get_contract      resolves every deployed contract
 *   * read-only invocation simulation for each core contract
 *
 * Usage:
 *   node scripts/verify-deployment.cjs [testnet|mainnet] [options]
 *
 * Options:
 *   --network <net>          testnet | mainnet            (default: testnet)
 *   --deployments <file>     Deployment JSON from deploy.sh
 *                            (default: .deployment-log-<net>.json)
 *   --deployments-md <file>  DEPLOYMENTS.md fallback      (default: DEPLOYMENTS.md)
 *   --source <identity>      soroban source identity      (default: admin)
 *   --rpc-url <url>          Override the RPC endpoint for the network
 *   --registry-name k=sym    Registry Symbol to look up for contract key `k`
 *   --skip-simulation        Skip the zero-value invocation simulation
 *   --json                   Emit a machine-readable summary instead of a report
 *   --help                   Show this help
 *
 * Exit codes: 0 = all passed, 1 = failures, 2 = could not run.
 *
 * Read-only: never signs or submits a state-changing transaction, never moves
 * funds. The simulation call uses `--send=no` when the CLI supports it.
 *
 * Env: SOROBAN_BIN overrides the soroban executable name/path.
 */
"use strict";

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const REPO_ROOT = path.resolve(__dirname, "..");

// key, contract_dir, wasm_name, readable_name, registry_symbol, required
const CONTRACTS = [
  { key: "aid", dir: "aid-contract", wasm: "aid_contract", readable: "Aid Contract", symbol: "aid", required: true },
  { key: "treasury", dir: "treasury-contract", wasm: "treasury_contract", readable: "Treasury Contract", symbol: "treasury", required: true },
  { key: "referral", dir: "referral-contract", wasm: "referral_contract", readable: "Referral Contract", symbol: "referral", required: true },
  { key: "registry", dir: "registry-contract", wasm: "registry_contract", readable: "Registry Contract", symbol: "registry", required: true },
  { key: "governance", dir: "governance-contract", wasm: "governance_contract", readable: "Governance Contract", symbol: "governance", required: false },
  { key: "oracle", dir: "oracle-contract", wasm: "oracle_contract", readable: "Oracle Contract", symbol: "oracle", required: false },
  { key: "payments", dir: "payments-contract", wasm: "payments_contract", readable: "Payments Contract", symbol: "payments", required: false },
  { key: "nft", dir: "nft-marketplace", wasm: "nft_marketplace", readable: "NFT Marketplace", symbol: "nft", required: false },
  { key: "access", dir: "access-control", wasm: "access_control", readable: "Access Control", symbol: "access", required: false },
  { key: "upgradeability", dir: "upgradeability", wasm: "upgradeability", readable: "Upgradeability", symbol: "upgradeability", required: false },
  { key: "rebalancer", dir: "rebalancer-contract", wasm: "rebalancer_contract", readable: "Rebalancer Contract", symbol: "rebalancer", required: false },
];

// key -> zero-value, read-only entry point used for the invocation simulation
const SIM_FN = {
  aid: "is_initialized",
  treasury: "referral_contract",
  referral: "get_treasury",
  registry: "list_names",
};

const CORE_KEYS = ["aid", "treasury", "referral", "registry"];

function usage() {
  console.log(`Trellis cross-contract deployment verification smoke test (Node runner).

Usage:
  node scripts/verify-deployment.cjs [testnet|mainnet] [options]

Options:
  --network <net>          testnet | mainnet            (default: testnet)
  --deployments <file>     Deployment JSON from deploy.sh
                           (default: .deployment-log-<net>.json)
  --deployments-md <file>  DEPLOYMENTS.md fallback      (default: DEPLOYMENTS.md)
  --source <identity>      soroban source identity      (default: admin)
  --rpc-url <url>          Override the RPC endpoint for the network
  --registry-name k=sym    Registry Symbol to look up for contract key \`k\`
  --skip-simulation        Skip the zero-value invocation simulation
  --json                   Emit a machine-readable summary instead of a report
  --help                   Show this help

Checks:
  * AidContract::get_treasury           == deployed TreasuryContract
  * TreasuryContract::referral_contract == deployed ReferralContract
  * ReferralContract::get_treasury      == deployed TreasuryContract
  * RegistryContract::get_contract      resolves every deployed contract
  * read-only invocation simulation for each core contract

Exit codes: 0 = all passed, 1 = failures, 2 = could not run.`);
  process.exit(0);
}

function die(msg, fix) {
  console.error(`VERIFY ERROR: ${msg}`);
  if (fix) console.error(`  Fix: ${fix}`);
  process.exit(2);
}

const oneLine = (s) => String(s).replace(/\s+/g, " ").trim().slice(0, 200);
const extractContractId = (s) => {
  const m = String(s).match(/C[A-Z0-9]{55}/);
  return m ? m[0] : "";
};
const byKey = (key) => CONTRACTS.find((c) => c.key === key);
const keyForDir = (dir) => (CONTRACTS.find((c) => c.dir === dir) || {}).key || "";
const keyForWasm = (wasm) => (CONTRACTS.find((c) => c.wasm === wasm) || {}).key || "";
const keyForReadable = (name) => (CONTRACTS.find((c) => c.readable === name) || {}).key || "";
const readableFor = (key) => (byKey(key) || {}).readable || key;

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------
const opts = {
  network: "testnet",
  deployments: "",
  deploymentsMd: path.join(REPO_ROOT, "DEPLOYMENTS.md"),
  source: "admin",
  rpcUrl: "",
  registryNames: {},
  skipSimulation: false,
  json: false,
};

const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) {
  const a = argv[i];
  switch (a) {
    case "--network": opts.network = argv[++i] || ""; break;
    case "--deployments": opts.deployments = argv[++i] || ""; break;
    case "--deployments-md": opts.deploymentsMd = argv[++i] || ""; break;
    case "--source": opts.source = argv[++i] || ""; break;
    case "--rpc-url": opts.rpcUrl = argv[++i] || ""; break;
    case "--registry-name": {
      const v = argv[++i] || "";
      const eq = v.indexOf("=");
      if (eq < 1) die("--registry-name requires key=symbol", "run with --help");
      opts.registryNames[v.slice(0, eq)] = v.slice(eq + 1);
      break;
    }
    case "--skip-simulation": opts.skipSimulation = true; break;
    case "--json": opts.json = true; break;
    case "--help": case "-h": usage(); break;
    case "testnet": case "mainnet": opts.network = a; break;
    default: die(`unknown argument: ${a}`, "run with --help");
  }
}

if (opts.network !== "testnet" && opts.network !== "mainnet") {
  die(`unsupported network '${opts.network}'`, "pass testnet or mainnet");
}

const symbolFor = (key) => opts.registryNames[key] || (byKey(key) || {}).symbol || "";

// ---------------------------------------------------------------------------
// Address resolution
// ---------------------------------------------------------------------------
const addresses = new Map();
const addrGet = (key) => addresses.get(key) || "";
const addrSet = (key, id) => addresses.set(key, id);

function resolveFromJson(file) {
  if (!file || !fs.existsSync(file)) return false;
  let doc;
  try { doc = JSON.parse(fs.readFileSync(file, "utf8")); } catch { return false; }
  const deps = Array.isArray(doc.deployments) ? doc.deployments : [];
  let found = 0;
  for (const d of deps) {
    const id = extractContractId(d && d.contract_id ? String(d.contract_id) : "");
    if (!id) continue;
    const key = keyForDir(d.contract_dir) || keyForWasm(d.wasm_name);
    if (!key) continue;
    addrSet(key, id);
    found++;
  }
  return found > 0;
}

function resolveFromMd(file) {
  if (!file || !fs.existsSync(file)) return false;
  const text = fs.readFileSync(file, "utf8");
  const cap = opts.network.charAt(0).toUpperCase() + opts.network.slice(1);
  const header = `## ${cap} Deployments`;
  let inSection = false;
  let found = 0;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === header) { inSection = true; continue; }
    if (inSection && /^## /.test(line)) break;
    if (!inSection || !line.startsWith("|")) continue;
    const cells = line.replace(/^\|/, "").replace(/\|$/, "").split("|").map((c) => c.trim());
    const name = cells[0];
    const addr = cells[1];
    if (!name || name === "Contract" || /^-+$/.test(name)) continue;
    if (/^C[A-Z0-9]{55}$/.test(addr || "")) {
      const key = keyForReadable(name);
      if (key) { addrSet(key, addr); found++; }
    }
  }
  return found > 0;
}

// ---------------------------------------------------------------------------
// soroban CLI
// ---------------------------------------------------------------------------
let sorobanBin = null;

function findSoroban() {
  if (sorobanBin) return sorobanBin;
  const candidates = process.env.SOROBAN_BIN
    ? [process.env.SOROBAN_BIN]
    : ["soroban", "soroban.exe", "soroban.cmd"];
  for (const bin of candidates) {
    const res = spawnSync(bin, ["--version"], { encoding: "utf8", timeout: 30000 });
    if (!res.error && (res.status === 0 || res.status === 1)) { sorobanBin = bin; return bin; }
  }
  return null;
}

function runSpawn(args) {
  const bin = findSoroban();
  if (!bin) return { rc: 127, out: "soroban CLI not found on PATH" };
  const res = spawnSync(bin, args, { encoding: "utf8", timeout: 90000, maxBuffer: 16 * 1024 * 1024 });
  const out = `${res.stdout || ""}${res.stderr || ""}`;
  if (res.error) return { rc: 1, out: `${out}${res.error.message}` };
  return { rc: res.status === null ? 1 : res.status, out };
}

let simFlag = null;
function detectSimFlag() {
  const res = runSpawn(["contract", "invoke", "--help"]);
  const help = res.out || "";
  if (help.includes("--send")) return "--send=no";
  if (help.includes("simulate-only")) return "--simulate-only";
  return "";
}

function invoke(id, fnArgs, { simulate = false } = {}) {
  const args = ["contract", "invoke", "--id", id, "--network", opts.network, "--source", opts.source];
  if (opts.rpcUrl) args.push("--rpc-url", opts.rpcUrl);
  if (simulate && simFlag) args.push(simFlag);
  args.push("--", ...fnArgs);
  return runSpawn(args);
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------
const checks = [];
let pass = 0;
let fail = 0;
let skip = 0;

function record(name, status, detail) {
  checks.push({ name, status, detail: detail || "" });
  if (status === "pass") pass++;
  else if (status === "fail") fail++;
  else skip++;
  if (!opts.json) {
    const label = status === "pass" ? "PASS" : status === "fail" ? "FAIL" : "SKIP";
    console.log(`  [${label}] ${name}`);
    if (detail) console.log(`        ${detail}`);
  }
}

const say = (s) => { if (!opts.json) console.log(s); };

function emitJson() {
  console.log(JSON.stringify({ network: opts.network, source: opts.source, pass, fail, skip, checks }));
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------
function checkLink(name, ckey, getter, expectKey) {
  const cid = addrGet(ckey);
  const expected = addrGet(expectKey);
  if (!expected) {
    record(name, "fail", `expected ${readableFor(expectKey)} address is missing from deployment data`);
    return;
  }
  const { rc, out } = invoke(cid, [getter]);
  if (rc !== 0) {
    record(name, "fail", `${readableFor(ckey)}::${getter}() invoke failed (rc=${rc}): ${oneLine(out)}`);
    return;
  }
  const got = extractContractId(out);
  if (!got) {
    record(name, "fail", `${readableFor(ckey)}::${getter}() returned no address (unset?) -> ${oneLine(out)}`);
    return;
  }
  if (got === expected) {
    record(name, "pass", `${readableFor(ckey)}::${getter}() -> ${got}  (matches ${readableFor(expectKey)})`);
  } else {
    record(name, "fail", `${readableFor(ckey)}::${getter}() -> ${got} but ${readableFor(expectKey)} is ${expected}; re-run ./scripts/initialize.sh with the correct wiring`);
  }
}

function checkRegistryEntry(key) {
  const registryId = addrGet("registry");
  const sym = symbolFor(key);
  const expected = addrGet(key);
  if (!sym) {
    record(`registry:${key}`, "fail", `no registry Symbol mapping for '${key}'; pass --registry-name ${key}=<symbol>`);
    return;
  }
  const { rc, out } = invoke(registryId, ["get_contract", "--name", sym]);
  if (rc !== 0) {
    record(`registry:${key}`, "fail", `RegistryContract::get_contract(${sym}) not found -> register it: soroban contract invoke --id ${registryId} --network ${opts.network} --source ${opts.source} -- set_contract --caller ${opts.source} --name ${sym} --address ${expected} --version 1`);
    return;
  }
  const got = extractContractId(out);
  if (!got) {
    record(`registry:${key}`, "fail", `RegistryContract::get_contract(${sym}) returned no address -> ${oneLine(out)}`);
    return;
  }
  if (got === expected) {
    record(`registry:${key}`, "pass", `RegistryContract::get_contract(${sym}) -> ${got}  (matches ${readableFor(key)})`);
  } else {
    record(`registry:${key}`, "fail", `RegistryContract::get_contract(${sym}) -> ${got} but ${readableFor(key)} is ${expected}; update the registry entry`);
  }
}

function checkSimulation(key) {
  const fn = SIM_FN[key];
  const cid = addrGet(key);
  if (!fn || !cid) return;
  const { rc, out } = invoke(cid, [fn], { simulate: true });
  if (rc === 0) {
    record(`simulate:${key}::${fn}`, "pass", `${readableFor(key)} accepted the simulated call (no value, no state change)`);
  } else {
    record(`simulate:${key}::${fn}`, "fail", `${readableFor(key)}::${fn}() simulation failed (rc=${rc}): ${oneLine(out)}`);
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------
say("========================================");
say("Trellis deployment verification (node)");
say(`Network:  ${opts.network}`);
say(`Source:   ${opts.source}`);
say("========================================");
say("");

if (!findSoroban()) {
  die("soroban CLI not found on PATH", "cargo install --locked soroban-cli (or set SOROBAN_BIN)");
}

if (!opts.deployments) opts.deployments = path.join(REPO_ROOT, `.deployment-log-${opts.network}.json`);

say("[1/5] Resolving deployed addresses...");
let resolvedFrom = "";
if (resolveFromJson(opts.deployments)) resolvedFrom = opts.deployments;
else if (resolveFromMd(opts.deploymentsMd)) resolvedFrom = `${opts.deploymentsMd} (markdown fallback)`;

if (!resolvedFrom) {
  die(`${opts.network} contracts are not deployed yet (no addresses in ${opts.deployments} or ${opts.deploymentsMd})`,
    `run ./scripts/deploy.sh ${opts.network} then ./scripts/record-deployments.sh ${opts.network}`);
}

say(`  source of truth: ${resolvedFrom}`);
for (const key of CORE_KEYS) {
  say(`  - ${readableFor(key)}: ${addrGet(key) || "(missing)"}`);
}
say("");

for (const key of CORE_KEYS) {
  const id = addrGet(key);
  if (!id) {
    record(`address:${key}`, "fail", `${readableFor(key)} address missing from deployment data; run ./scripts/deploy.sh ${opts.network} and ./scripts/record-deployments.sh ${opts.network}`);
  } else {
    record(`address:${key}`, "pass", `${readableFor(key)} -> ${id}`);
  }
}

if (fail > 0) {
  say("Cannot continue: required addresses are missing.");
  if (opts.json) emitJson();
  process.exit(1);
}

say("");
say("[2/5] Cross-contract linkage...");
checkLink("link:aid.get_treasury==treasury", "aid", "get_treasury", "treasury");
checkLink("link:treasury.referral_contract==referral", "treasury", "referral_contract", "referral");
checkLink("link:referral.get_treasury==treasury", "referral", "get_treasury", "treasury");

say("");
say("[3/5] Registry coverage...");
for (const key of Array.from(addresses.keys()).sort()) {
  checkRegistryEntry(key);
}

say("");
if (opts.skipSimulation) {
  say("[4/5] Invocation simulation skipped (--skip-simulation).");
} else {
  say("[4/5] Invocation simulation (read-only, no value)...");
  simFlag = detectSimFlag();
  if (!simFlag) say("  note: this soroban CLI exposes no --send/simulate-only flag; invoking read-only getters directly.");
  for (const key of CORE_KEYS) checkSimulation(key);
}

say("");
if (opts.json) {
  emitJson();
} else {
  say("========================================");
  say(`Result: ${pass} passed, ${fail} failed`);
  if (fail === 0) say(`All cross-contract checks passed for ${opts.network}.`);
  else say(`Fix each FAIL line above, then re-run: node scripts/verify-deployment.cjs ${opts.network}`);
  say("========================================");
}

process.exit(fail === 0 ? 0 : 1);
