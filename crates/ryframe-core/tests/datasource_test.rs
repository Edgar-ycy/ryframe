use ryframe_core::DataSourceManager;

#[test]
fn test_datasource_manager_new_empty() {
    let m = DataSourceManager::new();
    assert!(m.is_empty());
    assert_eq!(m.len(), 0);
    assert!(m.names().is_empty());
}

#[test]
fn test_datasource_manager_default() {
    let m = DataSourceManager::default();
    assert!(m.is_empty());
}
