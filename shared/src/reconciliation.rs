use soroban_sdk::{contracttype, Env, String, Vec};

/// Represents a state record as observed by a specific source
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRecord {
    pub id: u64,
    pub amount: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DriftType {
    MissingRecord,
    DuplicateRecord,
    StaleRecord,
    InconsistentBalance,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriftItem {
    pub record_id: u64,
    pub drift_type: DriftType,
    pub description: String,
    pub repair_guidance: String,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationReport {
    pub total_records_scanned: u32,
    pub drifts_detected: Vec<DriftItem>,
}

/// Runs a dry-run reconciliation across ledger, database, and user-facing records.
/// Checks for invariants and returns a report with detected drift and repair guidance.
pub fn run_reconciliation(
    env: &Env,
    ledger_records: Vec<SourceRecord>,
    db_records: Vec<SourceRecord>,
    user_records: Vec<SourceRecord>,
    current_timestamp: u64,
) -> ReconciliationReport {
    let mut drifts = Vec::new(env);
    let mut total_scanned = 0;
    
    // A simple O(N^2) or linear check approach. Since Soroban Vec doesn't have a HashMap, we'll iterate.
    // For large lists, an off-chain indexer would use a real db, but for contract-level / dry-run logic:
    
    // Check Ledger against DB and User
    let mut i = 0;
    while i < ledger_records.len() {
        let l_rec = ledger_records.get(i).unwrap();
        total_scanned += 1;
        
        let mut found_in_db = false;
        let mut db_rec_match = l_rec.clone();
        
        let mut db_count = 0;
        let mut j = 0;
        while j < db_records.len() {
            let d_rec = db_records.get(j).unwrap();
            if d_rec.id == l_rec.id {
                found_in_db = true;
                db_rec_match = d_rec;
                db_count += 1;
            }
            j += 1;
        }
        
        if db_count > 1 {
            drifts.push_back(DriftItem {
                record_id: l_rec.id,
                drift_type: DriftType::DuplicateRecord,
                description: String::from_str(env, "Record duplicated in database"),
                repair_guidance: String::from_str(env, "Remove duplicates in DB keeping the latest timestamp"),
            });
        }
        
        if !found_in_db {
            drifts.push_back(DriftItem {
                record_id: l_rec.id,
                drift_type: DriftType::MissingRecord,
                description: String::from_str(env, "Ledger record missing in database"),
                repair_guidance: String::from_str(env, "Re-sync database from ledger events"),
            });
        } else {
            // Check balance consistency
            if db_rec_match.amount != l_rec.amount {
                drifts.push_back(DriftItem {
                    record_id: l_rec.id,
                    drift_type: DriftType::InconsistentBalance,
                    description: String::from_str(env, "Database amount does not match ledger"),
                    repair_guidance: String::from_str(env, "Update database amount to match ledger canonical state"),
                });
            }
            
            // Check stale record (e.g. lag > 1000 units)
            if l_rec.timestamp > db_rec_match.timestamp + 1000 {
                drifts.push_back(DriftItem {
                    record_id: l_rec.id,
                    drift_type: DriftType::StaleRecord,
                    description: String::from_str(env, "Database record is stale compared to ledger"),
                    repair_guidance: String::from_str(env, "Trigger a fast-forward sync for this record"),
                });
            }
        }
        
        // Similarly for user records
        let mut found_in_user = false;
        let mut user_rec_match = l_rec.clone();
        
        let mut k = 0;
        while k < user_records.len() {
            let u_rec = user_records.get(k).unwrap();
            if u_rec.id == l_rec.id {
                found_in_user = true;
                user_rec_match = u_rec;
            }
            k += 1;
        }
        
        if !found_in_user {
            // Might be okay if user hasn't refreshed, but flag as missing in cache
            drifts.push_back(DriftItem {
                record_id: l_rec.id,
                drift_type: DriftType::MissingRecord,
                description: String::from_str(env, "Ledger record missing in user view"),
                repair_guidance: String::from_str(env, "Invalidate user cache for this record"),
            });
        } else if user_rec_match.amount != l_rec.amount {
            drifts.push_back(DriftItem {
                record_id: l_rec.id,
                drift_type: DriftType::InconsistentBalance,
                description: String::from_str(env, "User view amount does not match ledger"),
                repair_guidance: String::from_str(env, "Force client refresh of balances"),
            });
        }
        
        i += 1;
    }
    
    // Also check if there are DB records not in ledger
    let mut j = 0;
    while j < db_records.len() {
        let d_rec = db_records.get(j).unwrap();
        let mut found_in_ledger = false;
        
        let mut i = 0;
        while i < ledger_records.len() {
            if ledger_records.get(i).unwrap().id == d_rec.id {
                found_in_ledger = true;
                break;
            }
            i += 1;
        }
        
        if !found_in_ledger {
            drifts.push_back(DriftItem {
                record_id: d_rec.id,
                drift_type: DriftType::MissingRecord,
                description: String::from_str(env, "Database record missing in ledger (orphan)"),
                repair_guidance: String::from_str(env, "Archive or delete orphan database record"),
            });
            total_scanned += 1;
        }
        
        j += 1;
    }

    ReconciliationReport {
        total_records_scanned: total_scanned,
        drifts_detected: drifts,
    }
}
