//! Stable deterministic pagination and filtering for rapidly changing datasets (Issue #42).
//!
//! # Problem
//! Offset-based pagination (`limit`, `offset`) produces unstable results when datasets change
//! concurrently:
//! - Inserting a new record shifts existing records right, causing paginating users to see duplicate entries.
//! - Deleting, settling, or hiding records shifts existing records left, causing paginating users to skip entries.
//!
//! # Solution: Keyset (Cursor-Based) Pagination
//! This module implements deterministic keyset pagination where:
//! 1. Every record has a unique, monotonically ordered primary key or compound sequence key `K` (e.g., `u64` ID).
//! 2. The client provides `cursor = Option<K>`, representing the last seen key.
//! 3. The query seeks strictly beyond `cursor` (e.g., `k > cursor` for ascending order).
//! 4. Concurrently inserted records receive `id > cursor` and appear in future pages without disrupting earlier positions.
//! 5. Concurrently deleted, settled, or hidden records do not alter the ordering or absolute positions of unvisited keys.
//! 6. Gas limit protection: `max_scan` bounds total items inspected per call, returning a partial page with `next_cursor`
//!    when sparse filtering occurs, preventing out-of-gas transaction aborts.

use soroban_sdk::{contracttype, Env, Vec};
use crate::errors::Error;

/// Default page size when client requests 0.
pub const DEFAULT_PAGE_LIMIT: u32 = 20;

/// Maximum allowed page limit to bound compute and memory budget.
pub const MAX_PAGE_LIMIT: u32 = 100;

/// Maximum sequence/index elements to scan during one query before yielding.
/// Bounds worst-case execution gas for sparse filter predicates.
pub const DEFAULT_MAX_SCAN: u32 = 256;

/// Sort order direction.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Direction {
    Ascending,
    Descending,
}

/// Request parameters for stable keyset pagination.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageRequest {
    /// Last seen ID from the previous page. `None` starts from the beginning.
    pub cursor: Option<u64>,
    /// Maximum items to return on this page.
    pub limit: u32,
    /// Direction of traversal.
    pub direction: Direction,
}

impl PageRequest {
    pub fn new(cursor: Option<u64>, limit: u32, direction: Direction) -> Self {
        let effective_limit = if limit == 0 {
            DEFAULT_PAGE_LIMIT
        } else if limit > MAX_PAGE_LIMIT {
            MAX_PAGE_LIMIT
        } else {
            limit
        };
        Self {
            cursor,
            limit: effective_limit,
            direction,
        }
    }

    pub fn ascending(cursor: Option<u64>, limit: u32) -> Self {
        Self::new(cursor, limit, Direction::Ascending)
    }

    pub fn descending(cursor: Option<u64>, limit: u32) -> Self {
        Self::new(cursor, limit, Direction::Descending)
    }
}

/// Stable paginated response container.
#[derive(Clone)]
pub struct PageResponse<T> {
    /// Items matched and returned for this page.
    pub items: Vec<T>,
    /// Deterministic cursor to pass to the next request. `None` if result set is exhausted.
    pub next_cursor: Option<u64>,
    /// Whether additional elements exist beyond this page.
    pub has_more: bool,
    /// Total elements inspected during this page evaluation (for gas & telemetry transparency).
    pub scanned_count: u32,
}

/// Helper function to perform stable keyset pagination over a monotonic ID range `[0..total_count)`.
///
/// # Arguments
/// - `env`: Soroban environment.
/// - `total_count`: Total allocated monotonic ID counter.
/// - `request`: Keyset pagination request (`cursor`, `limit`, `direction`).
/// - `max_scan`: Upper bound on items evaluated in this call (gas protection).
/// - `fetch_and_filter`: Callback `(id) -> Option<T>` returning `Some(item)` if the item exists and satisfies filters.
pub fn paginate_id_range<T, F>(
    env: &Env,
    total_count: u64,
    request: &PageRequest,
    max_scan: u32,
    fetch_and_filter: F,
) -> Result<PageResponse<T>, Error>
where
    T: Clone + soroban_sdk::IntoVal<soroban_sdk::Env, soroban_sdk::Val> + soroban_sdk::TryFromVal<soroban_sdk::Env, soroban_sdk::Val>,
    F: Fn(u64) -> Option<T>,
{
    let limit = if request.limit == 0 {
        DEFAULT_PAGE_LIMIT
    } else {
        request.limit.min(MAX_PAGE_LIMIT)
    };

    let mut items = Vec::new(env);
    let mut scanned = 0u32;
    let mut last_processed_id: Option<u64> = None;
    let mut has_more = false;

    match request.direction {
        Direction::Ascending => {
            // Start from cursor + 1 if cursor is provided, else 0
            let start_id = match request.cursor {
                Some(c) => c.saturating_add(1),
                None => 0,
            };

            let mut curr_id = start_id;
            while curr_id < total_count && (items.len() as u32) < limit && scanned < max_scan {
                scanned += 1;
                last_processed_id = Some(curr_id);

                if let Some(item) = fetch_and_filter(curr_id) {
                    items.push_back(item);
                }

                curr_id += 1;
            }

            if curr_id < total_count {
                has_more = true;
            }
        }
        Direction::Descending => {
            // Start from cursor - 1 if cursor is provided, else total_count - 1
            if total_count > 0 {
                let start_id = match request.cursor {
                    Some(c) => {
                        if c == 0 {
                            // At lowest boundary, cannot go lower
                            total_count
                        } else {
                            (c - 1).min(total_count - 1)
                        }
                    }
                    None => total_count - 1,
                };

                if start_id < total_count {
                    let mut curr_id = start_id;
                    let mut done = false;

                    while !done && (items.len() as u32) < limit && scanned < max_scan {
                        scanned += 1;
                        last_processed_id = Some(curr_id);

                        if let Some(item) = fetch_and_filter(curr_id) {
                            items.push_back(item);
                        }

                        if curr_id == 0 {
                            done = true;
                        } else {
                            curr_id -= 1;
                        }
                    }

                    if !done {
                        has_more = true;
                    }
                }
            }
        }
    }

    let next_cursor = if has_more {
        last_processed_id
    } else {
        None
    };

    Ok(PageResponse {
        items,
        next_cursor,
        has_more: next_cursor.is_some(),
        scanned_count: scanned,
    })
}

/// Helper function to perform stable keyset pagination over a sorted vector of IDs `Vec<u64>`.
pub fn paginate_id_list<T, F>(
    env: &Env,
    ids: &Vec<u64>,
    request: &PageRequest,
    max_scan: u32,
    fetch_and_filter: F,
) -> Result<PageResponse<T>, Error>
where
    T: Clone + soroban_sdk::IntoVal<soroban_sdk::Env, soroban_sdk::Val> + soroban_sdk::TryFromVal<soroban_sdk::Env, soroban_sdk::Val>,
    F: Fn(u64) -> Option<T>,
{
    let limit = if request.limit == 0 {
        DEFAULT_PAGE_LIMIT
    } else {
        request.limit.min(MAX_PAGE_LIMIT)
    };

    let total_len = ids.len();
    let mut items = Vec::new(env);
    let mut scanned = 0u32;
    let mut last_matched_id: Option<u64> = None;
    let mut has_more = false;

    // Find the starting index in the sorted list matching cursor
    let mut start_index = 0u32;
    if let Some(cursor_id) = request.cursor {
        // Binary search or linear scan to find position strictly after cursor_id
        let mut i = 0u32;
        while i < total_len {
            if ids.get(i).unwrap_or(0) > cursor_id {
                start_index = i;
                break;
            }
            i += 1;
        }
        if i == total_len {
            // Cursor is past the end of the list
            return Ok(PageResponse {
                items,
                next_cursor: None,
                has_more: false,
                scanned_count: 0,
            });
        }
    }

    let mut idx = start_index;
    while idx < total_len && (items.len() as u32) < limit && scanned < max_scan {
        let id = ids.get(idx).unwrap_or(0);
        scanned += 1;
        last_matched_id = Some(id);

        if let Some(item) = fetch_and_filter(id) {
            items.push_back(item);
        }

        idx += 1;
    }

    if idx < total_len {
        has_more = true;
    }

    let next_cursor = if has_more {
        last_matched_id
    } else {
        None
    };

    Ok(PageResponse {
        items,
        next_cursor,
        has_more,
        scanned_count: scanned,
    })
}
