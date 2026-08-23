use super::process::child_command;

#[test]
fn nested_commands_remove_xtask_cargo_package_context() {
    let command = child_command("cargo");
    for expected in ["CARGO_MANIFEST_DIR", "CARGO_MANIFEST_PATH"] {
        let value = command
            .get_envs()
            .find(|(name, _)| *name == expected)
            .unwrap_or_else(|| panic!("缺少环境移除标记：{expected}"))
            .1;
        assert!(value.is_none());
    }
}
