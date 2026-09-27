#!/usr/bin/env node
/**
 * Validates changelog/entries.json against changelog/schema.json (Issue #126).
 *
 * Deliberately dependency-free (no ajv, no external JSON Schema library) so
 * it runs on a bare `node` with nothing installed — matching this repo's
 * other one-off Node scripts (verify-deployment.cjs, push-issues.js). It
 * implements exactly the subset of JSON Schema that changelog/schema.json
 * actually uses: type/enum/pattern/required/additionalProperties and the
 * one `allOf`/`if`/`then` conditional (breaking => non-empty migration_notes).
 *
 * Usage:
 *   node scripts/validate-changelog.mjs [--entries <file>] [--schema <file>]
 *
 * Exit codes: 0 = valid, 1 = invalid, 2 = could not run (missing/malformed file).
 */
"use strict";

const fs = require("fs");
const path = require("path");

function parseArgs(argv) {
  const out = { entries: "changelog/entries.json", schema: "changelog/schema.json" };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--entries" && argv[i + 1]) out.entries = argv[++i];
    else if (argv[i] === "--schema" && argv[i + 1]) out.schema = argv[++i];
  }
  return out;
}

function readJson(filePath) {
  const abs = path.resolve(filePath);
  if (!fs.existsSync(abs)) {
    throw new Error(`file not found: ${filePath}`);
  }
  try {
    return JSON.parse(fs.readFileSync(abs, "utf8"));
  } catch (err) {
    throw new Error(`invalid JSON in ${filePath}: ${err.message}`);
  }
}

/** Validates one entry against schema.items. Returns an array of error strings. */
function validateEntry(entry, itemSchema, index) {
  const errors = [];
  const prefix = `entries[${index}]`;

  if (typeof entry !== "object" || entry === null || Array.isArray(entry)) {
    return [`${prefix}: expected an object`];
  }

  for (const key of itemSchema.required || []) {
    if (!(key in entry)) errors.push(`${prefix}: missing required field "${key}"`);
  }

  if (itemSchema.additionalProperties === false) {
    const allowed = new Set(Object.keys(itemSchema.properties || {}));
    for (const key of Object.keys(entry)) {
      if (!allowed.has(key)) errors.push(`${prefix}: unexpected field "${key}"`);
    }
  }

  for (const [key, propSchema] of Object.entries(itemSchema.properties || {})) {
    if (!(key in entry)) continue;
    const value = entry[key];
    errors.push(...validateValue(value, propSchema, `${prefix}.${key}`));
  }

  for (const rule of itemSchema.allOf || []) {
    const cond = rule.if && rule.if.properties;
    if (!cond) continue;
    const matches = Object.entries(cond).every(
      ([k, spec]) => entry[k] === spec.const
    );
    if (matches && rule.then && rule.then.properties) {
      for (const [k, spec] of Object.entries(rule.then.properties)) {
        if (typeof spec.minLength === "number") {
          const v = entry[k];
          if (typeof v !== "string" || v.length < spec.minLength) {
            errors.push(
              `${prefix}: "${k}" must be non-empty when this entry's condition holds (e.g. impact = "breaking" requires migration_notes)`
            );
          }
        }
      }
    }
  }

  return errors;
}

function validateValue(value, schema, label) {
  const errors = [];
  const types = Array.isArray(schema.type) ? schema.type : [schema.type];
  const actual = value === null ? "null" : Array.isArray(value) ? "array" : typeof value;
  if (schema.type && !types.includes(actual) && !(actual === "number" && types.includes("integer") && Number.isInteger(value))) {
    errors.push(`${label}: expected type ${types.join(" | ")}, got ${actual}`);
    return errors; // further checks would be meaningless on the wrong type
  }
  if (schema.enum && !schema.enum.includes(value)) {
    errors.push(`${label}: "${value}" is not one of [${schema.enum.join(", ")}]`);
  }
  if (typeof value === "string") {
    if (typeof schema.minLength === "number" && value.length < schema.minLength) {
      errors.push(`${label}: must be at least ${schema.minLength} character(s)`);
    }
    if (typeof schema.maxLength === "number" && value.length > schema.maxLength) {
      errors.push(`${label}: must be at most ${schema.maxLength} characters`);
    }
    if (schema.pattern && !new RegExp(schema.pattern).test(value)) {
      errors.push(`${label}: "${value}" does not match required pattern ${schema.pattern}`);
    }
  }
  return errors;
}

function main() {
  const { entries: entriesPath, schema: schemaPath } = parseArgs(process.argv.slice(2));

  let schema;
  let entries;
  try {
    schema = readJson(schemaPath);
    entries = readJson(entriesPath);
  } catch (err) {
    console.error(`✗ ${err.message}`);
    process.exit(2);
  }

  if (schema.type !== "array" || !schema.items) {
    console.error(`✗ ${schemaPath}: expected a top-level {"type": "array", "items": {...}} schema`);
    process.exit(2);
  }
  if (!Array.isArray(entries)) {
    console.error(`✗ ${entriesPath}: expected a top-level JSON array`);
    process.exit(2);
  }

  const errors = entries.flatMap((entry, i) => validateEntry(entry, schema.items, i));

  if (errors.length > 0) {
    console.error(`✗ ${entriesPath} failed validation against ${schemaPath}:\n`);
    for (const err of errors) console.error(`  - ${err}`);
    console.error(`\n${errors.length} error(s) in ${entries.length} entrie(s).`);
    process.exit(1);
  }

  console.log(`✓ ${entriesPath}: ${entries.length} entrie(s) valid against ${schemaPath}.`);
  process.exit(0);
}

main();
