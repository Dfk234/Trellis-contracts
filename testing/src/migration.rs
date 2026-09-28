//! Migration Simulation Framework
//!
//! Provides utilities for dry-running storage migrations,
//! classifying records into success, failure, and manual intervention categories,
//! and ensuring no irreversible writes occur during the dry run.

use soroban_sdk::{Env, String, Vec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationStatus {
    /// The record can be successfully migrated automatically.
    Success,
    /// The record fails validation or conversion.
    Failure(String),
    /// The record requires manual administrative handling.
    ManualHandlingRequired(String),
}

/// A report summarizing the dry-run migration.
#[derive(Debug, Clone)]
pub struct MigrationReport {
    pub total_records: u32,
    pub successful: u32,
    pub failed: u32,
    pub manual_handling: u32,
    pub failed_details: Vec<(u32, String)>,
    pub manual_details: Vec<(u32, String)>,
    pub partial_migration: bool,
    pub rollback_notes: String,
}

impl MigrationReport {
    pub fn new(env: &Env) -> Self {
        Self {
            total_records: 0,
            successful: 0,
            failed: 0,
            manual_handling: 0,
            failed_details: Vec::new(env),
            manual_details: Vec::new(env),
            partial_migration: false,
            rollback_notes: String::from_str(env, ""),
        }
    }
}

/// Dry-run a migration over a set of mock records.
pub struct MigrationSimulator<'a, T, U> {
    env: &'a Env,
    records: Vec<T>,
    migration_fn: fn(&Env, &T) -> Result<U, MigrationStatus>,
}

impl<'a, T, U> MigrationSimulator<'a, T, U> 
where
    T: Clone,
{
    pub fn new(env: &'a Env, records: Vec<T>, migration_fn: fn(&Env, &T) -> Result<U, MigrationStatus>) -> Self {
        Self {
            env,
            records,
            migration_fn,
        }
    }

    pub fn run(&self) -> MigrationReport {
        let mut report = MigrationReport::new(self.env);
        report.total_records = self.records.len();

        for (i, record) in self.records.iter().enumerate() {
            let res = (self.migration_fn)(self.env, &record);
            match res {
                Ok(_) => {
                    report.successful += 1;
                }
                Err(MigrationStatus::Failure(reason)) => {
                    report.failed += 1;
                    report.failed_details.push_back((i as u32, reason));
                }
                Err(MigrationStatus::ManualHandlingRequired(reason)) => {
                    report.manual_handling += 1;
                    report.manual_details.push_back((i as u32, reason));
                }
                Err(MigrationStatus::Success) => {
                    report.successful += 1;
                }
            }
        }
        
        if report.failed > 0 || report.manual_handling > 0 {
            report.partial_migration = true;
            report.rollback_notes = String::from_str(self.env, "Failures encountered. Partial migration requires rollback or manual fixes before completion.");
        }
        
        report
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{Env, String};

    #[derive(Clone)]
    struct OldRecord {
        id: u32,
        data: u32,
        corrupted: bool,
    }

    #[derive(Clone)]
    struct NewRecord {
        id: u32,
        data: u64,
    }

    fn dummy_migration(env: &Env, old: &OldRecord) -> Result<NewRecord, MigrationStatus> {
        if old.corrupted {
            return Err(MigrationStatus::Failure(String::from_str(env, "Data corrupted")));
        }
        if old.data == 999 {
            return Err(MigrationStatus::ManualHandlingRequired(String::from_str(env, "Special flag requires manual processing")));
        }

        Ok(NewRecord {
            id: old.id,
            data: old.data as u64,
        })
    }

    #[test]
    fn test_migration_simulation() {
        let env = Env::default();

        let mut records = Vec::new(&env);
        // Valid migration
        records.push_back(OldRecord { id: 1, data: 100, corrupted: false });
        records.push_back(OldRecord { id: 2, data: 200, corrupted: false });
        
        // Invalid old data
        records.push_back(OldRecord { id: 3, data: 300, corrupted: true });
        
        // Manual handling
        records.push_back(OldRecord { id: 4, data: 999, corrupted: false });

        let simulator = MigrationSimulator::new(&env, records, dummy_migration);
        let report = simulator.run();

        assert_eq!(report.total_records, 4);
        assert_eq!(report.successful, 2);
        assert_eq!(report.failed, 1);
        assert_eq!(report.manual_handling, 1);
        assert_eq!(report.partial_migration, true);
        assert_eq!(report.rollback_notes, String::from_str(&env, "Failures encountered. Partial migration requires rollback or manual fixes before completion."));
    }
}
