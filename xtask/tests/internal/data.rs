use super::data::{target_inventory_arguments, tenant_data_arguments};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn maintenance_operations_map_to_existing_private_binary_commands() {
    assert_eq!(
        tenant_data_arguments("backup", &strings(&["status", "--id", "b1"])).unwrap(),
        strings(&["backup-status", "--id", "b1"])
    );
    assert_eq!(
        tenant_data_arguments("restore", &strings(&["verify", "--id", "r1"])).unwrap(),
        strings(&["restore-verify", "--id", "r1"])
    );
    assert_eq!(
        target_inventory_arguments(&strings(&["inventory", "--target", "tenant-a"])).unwrap(),
        strings(&["target-inventory", "--target", "tenant-a"])
    );
}
