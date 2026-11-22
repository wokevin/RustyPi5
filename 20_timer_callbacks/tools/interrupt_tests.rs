// SPDX-License-Identifier: MIT OR Apache-2.0

#[path = "..\\kernel\\src\\cpu\\work_queue.rs"]
mod work_queue;
use work_queue::{Work, WorkQueue};

fn probe(sequence: u64) -> Work {
    Work::Probe { source_cpu: 1, sequence }
}

#[test]
fn empty_queue_has_no_work() {
    let mut queue = WorkQueue::<4>::new();
    assert_eq!(queue.pop(), None);
    assert_eq!(queue.rejected(), 0);
}

#[test]
fn work_is_processed_in_fifo_order() {
    let mut queue = WorkQueue::<4>::new();
    for sequence in 1..=4 { queue.push(probe(sequence)).unwrap(); }
    for sequence in 1..=4 { assert_eq!(queue.pop(), Some(probe(sequence))); }
    assert_eq!(queue.pop(), None);
}

#[test]
fn wraparound_reuses_slots_without_overwriting_work() {
    let mut queue = WorkQueue::<3>::new();
    for sequence in 1..=100 {
        queue.push(probe(sequence)).unwrap();
        queue.push(probe(sequence + 100)).unwrap();
        assert_eq!(queue.pop(), Some(probe(sequence)));
        queue.push(probe(sequence + 200)).unwrap();
        assert_eq!(queue.pop(), Some(probe(sequence + 100)));
        assert_eq!(queue.pop(), Some(probe(sequence + 200)));
    }
    assert_eq!(queue.pop(), None);
    assert_eq!(queue.rejected(), 0);
}

#[test]
fn full_queue_rejects_and_counts_without_losing_accepted_work() {
    let mut queue = WorkQueue::<1>::new();
    queue.push(probe(1)).unwrap();
    assert_eq!(queue.push(probe(2)), Err("Kernel work queue is full"));
    assert_eq!(queue.push(probe(3)), Err("Kernel work queue is full"));
    assert_eq!(queue.rejected(), 2);
    assert_eq!(queue.pop(), Some(probe(1)));
    queue.push(probe(4)).unwrap();
    assert_eq!(queue.pop(), Some(probe(4)));
    assert_eq!(queue.rejected(), 2);
}

#[test]
fn zero_capacity_rejects_without_dividing_by_zero() {
    let mut queue = WorkQueue::<0>::new();
    assert_eq!(queue.push(probe(1)), Err("Kernel work queue is full"));
    assert_eq!(queue.rejected(), 1);
    assert_eq!(queue.pop(), None);
}
