use soroban_sdk::{contracttype, Env, String, Vec};

use crate::health::{list_dependency_health, DependencyStatus};
use crate::jobs::{list_dead_letters, JobPayload};
use crate::reconciliation::{run_reconciliation, SourceRecord};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedactedDeadLetter {
    pub job_id: u64,
    pub attempts: u32,
    pub last_error: u32,
    pub failed_at_ledger: u32,
    pub investigation_link: String,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthCategory {
    pub name: String,
    pub count: u32,
    pub actionable: bool,
    pub investigation_link: String,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardReport {
    pub categories: Vec<HealthCategory>,
    pub redacted_dead_letters: Vec<RedactedDeadLetter>,
}

/// Generates an operational health dashboard summarizing unresolved failures,
/// stale jobs, reconciliation drift, and user-impacting incidents.
pub fn generate_dashboard(
    env: &Env,
    ledger_records: Vec<SourceRecord>,
    db_records: Vec<SourceRecord>,
    user_records: Vec<SourceRecord>,
    current_timestamp: u64,
) -> DashboardReport {
    let dead_letters = list_dead_letters(env, None, 100);
    let unresolved_count = dead_letters.len();

    let mut redacted_dead_letters = Vec::new(env);
    for dl in dead_letters.into_iter() {
        let link = String::from_str(env, "logs/jobs/dead_letter");
        redacted_dead_letters.push_back(RedactedDeadLetter {
            job_id: dl.job_id,
            attempts: dl.attempts,
            last_error: dl.last_error,
            failed_at_ledger: dl.failed_at_ledger,
            investigation_link: link,
        });
    }

    let rec_report = run_reconciliation(
        env,
        ledger_records,
        db_records,
        user_records,
        current_timestamp,
    );
    let drift_count = rec_report.drifts_detected.len();

    let deps = list_dependency_health(env);
    let mut incident_count = 0;
    for dep in deps.iter() {
        if dep.status == DependencyStatus::Degraded || dep.status == DependencyStatus::Down {
            incident_count += 1;
        }
    }

    let mut categories = Vec::new(env);
    categories.push_back(HealthCategory {
        name: String::from_str(env, "Unresolved Failures & Stale Jobs"),
        count: unresolved_count,
        actionable: unresolved_count > 0,
        investigation_link: String::from_str(env, "dashboard/jobs"),
    });
    categories.push_back(HealthCategory {
        name: String::from_str(env, "Reconciliation Drift"),
        count: drift_count,
        actionable: drift_count > 0,
        investigation_link: String::from_str(env, "dashboard/reconciliation"),
    });
    categories.push_back(HealthCategory {
        name: String::from_str(env, "User-Impacting Incidents"),
        count: incident_count,
        actionable: incident_count > 0,
        investigation_link: String::from_str(env, "dashboard/incidents"),
    });

    DashboardReport {
        categories,
        redacted_dead_letters,
    }
}
