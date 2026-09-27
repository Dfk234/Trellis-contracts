#![cfg(test)]

extern crate std;

use soroban_sdk::{Env, Vec};
use crate::pagination::{
    paginate_id_list, paginate_id_range, Direction, PageRequest, DEFAULT_MAX_SCAN,
};

#[test]
fn test_sequential_ascending_pagination() {
    let env = Env::default();
    let total_count = 10u64;

    // Page 1: limit 4, cursor None
    let req1 = PageRequest::ascending(None, 4);
    let page1 = paginate_id_range(&env, total_count, &req1, DEFAULT_MAX_SCAN, |id| Some(id)).unwrap();
    assert_eq!(page1.items.len(), 4);
    assert_eq!(page1.items.get(0).unwrap(), 0);
    assert_eq!(page1.items.get(3).unwrap(), 3);
    assert_eq!(page1.next_cursor, Some(3));
    assert!(page1.has_more);

    // Page 2: limit 4, cursor Some(3)
    let req2 = PageRequest::ascending(page1.next_cursor, 4);
    let page2 = paginate_id_range(&env, total_count, &req2, DEFAULT_MAX_SCAN, |id| Some(id)).unwrap();
    assert_eq!(page2.items.len(), 4);
    assert_eq!(page2.items.get(0).unwrap(), 4);
    assert_eq!(page2.items.get(3).unwrap(), 7);
    assert_eq!(page2.next_cursor, Some(7));
    assert!(page2.has_more);

    // Page 3: limit 4, cursor Some(7)
    let req3 = PageRequest::ascending(page2.next_cursor, 4);
    let page3 = paginate_id_range(&env, total_count, &req3, DEFAULT_MAX_SCAN, |id| Some(id)).unwrap();
    assert_eq!(page3.items.len(), 2);
    assert_eq!(page3.items.get(0).unwrap(), 8);
    assert_eq!(page3.items.get(1).unwrap(), 9);
    assert_eq!(page3.next_cursor, None);
    assert!(!page3.has_more);
}

#[test]
fn test_stability_under_concurrent_inserts() {
    let env = Env::default();
    let mut total_count = 10u64;

    // Read Page 1 (IDs 0..4)
    let req1 = PageRequest::ascending(None, 5);
    let page1 = paginate_id_range(&env, total_count, &req1, DEFAULT_MAX_SCAN, |id| Some(id)).unwrap();
    assert_eq!(page1.items.len(), 5);
    assert_eq!(page1.next_cursor, Some(4));

    // Concurrently insert 5 new records (total_count becomes 15)
    total_count += 5;

    // Read Page 2 with cursor Some(4)
    let req2 = PageRequest::ascending(page1.next_cursor, 5);
    let page2 = paginate_id_range(&env, total_count, &req2, DEFAULT_MAX_SCAN, |id| Some(id)).unwrap();
    assert_eq!(page2.items.len(), 5);
    assert_eq!(page2.items.get(0).unwrap(), 5);
    assert_eq!(page2.items.get(4).unwrap(), 9);
    assert_eq!(page2.next_cursor, Some(9));

    // Read Page 3 (newly inserted records)
    let req3 = PageRequest::ascending(page2.next_cursor, 5);
    let page3 = paginate_id_range(&env, total_count, &req3, DEFAULT_MAX_SCAN, |id| Some(id)).unwrap();
    assert_eq!(page3.items.len(), 5);
    assert_eq!(page3.items.get(0).unwrap(), 10);
    assert_eq!(page3.items.get(4).unwrap(), 14);
    assert_eq!(page3.next_cursor, None);
}

#[test]
fn test_stability_under_concurrent_deletes_and_filtering() {
    let env = Env::default();
    let total_count = 10u64;

    // Simulate active status where items 1 and 3 are deleted/hidden
    let mut active = [true; 10];

    // Page 1
    let req1 = PageRequest::ascending(None, 3);
    let page1 = paginate_id_range(&env, total_count, &req1, DEFAULT_MAX_SCAN, |id| {
        if active[id as usize] {
            Some(id)
        } else {
            None
        }
    }).unwrap();
    assert_eq!(page1.items.len(), 3);
    assert_eq!(page1.items.get(0).unwrap(), 0);
    assert_eq!(page1.items.get(1).unwrap(), 1);
    assert_eq!(page1.items.get(2).unwrap(), 2);
    assert_eq!(page1.next_cursor, Some(2));

    // Now delete items 0 and 1
    active[0] = false;
    active[1] = false;

    // Page 2 resumes from cursor Some(2), seeking strictly > 2
    let req2 = PageRequest::ascending(page1.next_cursor, 3);
    let page2 = paginate_id_range(&env, total_count, &req2, DEFAULT_MAX_SCAN, |id| {
        if active[id as usize] {
            Some(id)
        } else {
            None
        }
    }).unwrap();

    assert_eq!(page2.items.len(), 3);
    assert_eq!(page2.items.get(0).unwrap(), 3);
    assert_eq!(page2.items.get(1).unwrap(), 4);
    assert_eq!(page2.items.get(2).unwrap(), 5);
    assert_eq!(page2.next_cursor, Some(5));
}

#[test]
fn test_descending_pagination() {
    let env = Env::default();
    let total_count = 10u64;

    // Page 1 descending: limit 4, cursor None (starts at 9)
    let req1 = PageRequest::descending(None, 4);
    let page1 = paginate_id_range(&env, total_count, &req1, DEFAULT_MAX_SCAN, |id| Some(id)).unwrap();
    assert_eq!(page1.items.len(), 4);
    assert_eq!(page1.items.get(0).unwrap(), 9);
    assert_eq!(page1.items.get(3).unwrap(), 6);
    assert_eq!(page1.next_cursor, Some(6));

    // Page 2 descending: cursor Some(6) -> items 5, 4, 3, 2
    let req2 = PageRequest::descending(page1.next_cursor, 4);
    let page2 = paginate_id_range(&env, total_count, &req2, DEFAULT_MAX_SCAN, |id| Some(id)).unwrap();
    assert_eq!(page2.items.len(), 4);
    assert_eq!(page2.items.get(0).unwrap(), 5);
    assert_eq!(page2.items.get(3).unwrap(), 2);
    assert_eq!(page2.next_cursor, Some(2));

    // Page 3 descending: cursor Some(2) -> items 1, 0
    let req3 = PageRequest::descending(page2.next_cursor, 4);
    let page3 = paginate_id_range(&env, total_count, &req3, DEFAULT_MAX_SCAN, |id| Some(id)).unwrap();
    assert_eq!(page3.items.len(), 2);
    assert_eq!(page3.items.get(0).unwrap(), 1);
    assert_eq!(page3.items.get(1).unwrap(), 0);
    assert_eq!(page3.next_cursor, None);
}

#[test]
fn test_paginate_id_list() {
    let env = Env::default();
    let mut ids = Vec::new(&env);
    ids.push_back(10);
    ids.push_back(25);
    ids.push_back(30);
    ids.push_back(45);
    ids.push_back(50);

    let req1 = PageRequest::ascending(None, 2);
    let page1 = paginate_id_list(&env, &ids, &req1, DEFAULT_MAX_SCAN, |id| Some(id * 2)).unwrap();
    assert_eq!(page1.items.len(), 2);
    assert_eq!(page1.items.get(0).unwrap(), 20); // 10 * 2
    assert_eq!(page1.items.get(1).unwrap(), 50); // 25 * 2
    assert_eq!(page1.next_cursor, Some(25));

    let req2 = PageRequest::ascending(page1.next_cursor, 2);
    let page2 = paginate_id_list(&env, &ids, &req2, DEFAULT_MAX_SCAN, |id| Some(id * 2)).unwrap();
    assert_eq!(page2.items.len(), 2);
    assert_eq!(page2.items.get(0).unwrap(), 60); // 30 * 2
    assert_eq!(page2.items.get(1).unwrap(), 90); // 45 * 2
    assert_eq!(page2.next_cursor, Some(45));
}
