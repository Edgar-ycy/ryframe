use std::path::Path;

use crate::contract::model;

#[test]
fn contract_export_resolves_cargo_target_against_the_backend_root() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    assert_eq!(
        model::export_target(&root, None),
        root.join("target/xtask-contract")
    );
    assert_eq!(
        model::export_target(&root, Some("target/isolated".into())),
        root.join("target/isolated")
    );
    let absolute = root.join(".local-tests/共享编译 target");
    assert_eq!(
        model::export_target(&root, Some(absolute.clone().into_os_string())),
        absolute
    );
}
