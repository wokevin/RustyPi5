// SPDX-License-Identifier: MIT OR Apache-2.0

#[path = "../kernel/src/cpu/topology.rs"]
mod topology;
use topology::{parse_madt, select_user_cpu, user_eligible, Topology};

fn madt(entries: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0u8; 44];
    bytes[..4].copy_from_slice(b"APIC");
    bytes.extend_from_slice(entries);
    bytes
}

#[test]
fn nonzero_boot_id_and_sparse_ids_are_not_table_indices() {
    let mut topology = Topology::<4>::new(17);
    for id in [42, 3, 17, 91] {
        topology.add(id, true).unwrap();
    }
    let topology = topology.finish().unwrap();
    assert_eq!(topology.ids(), &[17, 3, 42, 91]);
}

#[test]
fn disabled_and_hotplug_only_cpus_are_not_runnable() {
    let bytes = madt(&[0, 8, 0, 7, 2, 0, 0, 0, 0, 8, 1, 42, 1, 0, 0, 0]);
    let mut topology = Topology::<2>::new(42);
    parse_madt(&bytes, |id, enabled| topology.add(id, enabled)).unwrap();
    assert_eq!(topology.finish().unwrap().ids(), &[42]);
}

#[test]
fn duplicates_missing_boot_and_capacity_overflow_are_errors() {
    let mut topology = Topology::<2>::new(0);
    topology.add(0, true).unwrap();
    assert!(topology.add(0, true).is_err());
    topology.add(1, true).unwrap();
    assert!(topology.add(2, true).is_err());
    let mut topology = Topology::<2>::new(9);
    topology.add(1, true).unwrap();
    assert!(topology.finish().is_err());
    assert!(Topology::<0>::new(0).add(0, true).is_err());
}

#[test]
fn malformed_madt_entries_are_errors_not_partial_success() {
    for entries in [
        &[0u8][..],
        &[0, 0],
        &[0, 1],
        &[0, 8, 1],
        &[0, 7, 0, 0, 0, 0, 0],
        &[9, 2],
    ] {
        assert!(parse_madt(&madt(entries), |_, _| Ok(())).is_err());
    }
    assert!(parse_madt(&[0; 43], |_, _| Ok(())).is_err());
}

#[test]
fn enabled_x2apic_is_explicitly_unsupported() {
    let mut entry = [0; 16];
    entry[0] = 9;
    entry[1] = 16;
    entry[8] = 1;
    assert!(parse_madt(&madt(&entry), |_, _| Ok(())).is_err());
    entry[8] = 0;
    parse_madt(&madt(&entry), |_, _| Ok(())).unwrap();
}

#[test]
fn only_online_secondaries_can_run_users() {
    assert!(!user_eligible(0, true));
    assert!(!user_eligible(1, false));
    assert!(user_eligible(3, true));
    assert_eq!(
        select_user_cpu([(0, true), (1, false), (2, true)].into_iter()).unwrap(),
        2
    );
    assert!(select_user_cpu([(0, true)].into_iter()).is_err());
    assert!(select_user_cpu([(0, true), (1, false)].into_iter()).is_err());
}
