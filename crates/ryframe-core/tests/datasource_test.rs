use ryframe_core::{DataSourceContext, DataSourceManager};

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

#[test]
fn test_datasource_context_current_name_default() {
    let name = DataSourceContext::current_name();
    assert_eq!(name, "primary");
}

#[test]
fn test_datasource_context_try_current_name_default() {
    let name = DataSourceContext::try_current_name();
    let _ = name;
}
