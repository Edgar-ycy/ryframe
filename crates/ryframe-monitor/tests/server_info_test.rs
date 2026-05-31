use ryframe_monitor::server_info::ServerInfo;

#[test]
fn test_server_info() {
    let info = ServerInfo::collect();
    assert!(!info.os.is_empty());
    assert!(info.cpu_cores > 0);
    assert!(info.total_memory > 0.0);
    assert!(info.pid > 0);
    assert!(info.memory_usage >= 0.0 && info.memory_usage <= 100.0);

    let json = serde_json::to_value(&info).unwrap();
    assert!(json.get("os").is_some());
    assert!(json.get("cpu_cores").is_some());
}
