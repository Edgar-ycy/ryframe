use super::check::parse_change_surface_policy;

fn policy_source() -> String {
    include_str!("../fixtures/change_surface_policy.toml").to_owned()
}

#[test]
fn change_surface_policy_rejects_invalid_soft_thresholds() {
    for (source, expected) in [
        (
            policy_source().replace("warning_percent = 80", "warning_percent = 79"),
            "80% 提醒和 90% 高关注",
        ),
        (
            policy_source().replace(
                "backend_rust_hard_limit = 600",
                "backend_rust_hard_limit = 1",
            ),
            "80% 提醒 < 90% 高关注 < 硬上限",
        ),
    ] {
        let error = parse_change_surface_policy(&source)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn change_surface_policy_rejects_unsafe_or_duplicate_invalidation_paths() {
    let unsafe_path = policy_source().replace("path = \"Cargo.toml\"", "path = \"../Cargo.toml\"");
    let error = parse_change_surface_policy(&unsafe_path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("规范的正斜杠相对路径"), "{error}");

    let missing_prefix_slash = policy_source().replace("match = \"exact\"", "match = \"prefix\"");
    let error = parse_change_surface_policy(&missing_prefix_slash)
        .unwrap_err()
        .to_string();
    assert!(error.contains("必须以斜杠结尾"), "{error}");

    let empty_reason = policy_source().replace("reason = \"测试\"", "reason = \" \"");
    let error = parse_change_surface_policy(&empty_reason)
        .unwrap_err()
        .to_string();
    assert!(error.contains("必须说明原因"), "{error}");

    let duplicate = format!(
        "{}\n{}",
        policy_source(),
        r#"
[[full_invalidation_paths]]
repository = "backend"
match = "exact"
path = "Cargo.toml"
reason = "重复"
"#
    );
    let error = parse_change_surface_policy(&duplicate)
        .unwrap_err()
        .to_string();
    assert!(error.contains("重复配置"), "{error}");
}
