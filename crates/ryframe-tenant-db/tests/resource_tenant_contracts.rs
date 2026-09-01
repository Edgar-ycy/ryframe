use std::collections::BTreeSet;

use ryframe_tenant_db::generated::{MIGRATION_NAMES, migrations};

#[test]
fn generated_migration_registry_is_complete_sorted_and_unique() {
    let migrations = migrations();
    assert_eq!(migrations.len(), MIGRATION_NAMES.len());
    assert!(MIGRATION_NAMES.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        MIGRATION_NAMES.len(),
        MIGRATION_NAMES
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
    );
}
