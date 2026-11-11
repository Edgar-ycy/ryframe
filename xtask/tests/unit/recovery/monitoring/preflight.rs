use super::run_protocol;
use serde_json::json;

#[test]
fn monitoring_protocol_rejects_retired_envelopes_wrong_types_and_relative_paths() {
    let backend = crate::recovery::fixture_control::git_root(
        &crate::workspace::root_dir(),
        "监控测试协调后端",
    )
    .unwrap();
    let request = json!({
        "format_version":1,
        "kind":"monitoring-preflight",
        "backend":backend,
        "runtime":"relative-runtime.json",
        "target":backend.join(".local-tests/target.json")
    });

    for (field, value, diagnostic) in [
        ("native_tool", json!({}), "字段缺失或包含未知字段"),
        ("write", json!(true), "字段缺失或包含未知字段"),
        ("format_version", json!(true), "版本或类型无效"),
        ("kind", json!("other"), "版本或类型无效"),
        ("backend", json!(1), "必须为字符串"),
        ("backend", json!("relative-backend"), "路径必须是绝对路径"),
        (
            "runtime",
            json!("relative-runtime.json"),
            "路径必须是绝对路径",
        ),
    ] {
        let mut invalid = request.clone();
        invalid[field] = value;
        let error = run_protocol(&invalid).unwrap_err().to_string();
        assert!(error.contains(diagnostic), "{field}: {error}");
    }
}
