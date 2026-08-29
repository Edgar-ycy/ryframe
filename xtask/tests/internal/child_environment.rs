use std::{fs, path::Path, process::Command};

use super::{
    check::{ResourceWorkspaceProfile, resource_workspace_environment_for_profile},
    process::{child_command, run_with_env_removed},
};

const PROFILE_ENV: &str = "RYFRAME_RESOURCE_WORKSPACE_PROFILE";
const HELPER_ROLE_ENV: &str = "RYFRAME_XTASK_RESOURCE_PROFILE_TEST_ROLE";
const RESULT_FILE_ENV: &str = "RYFRAME_XTASK_RESOURCE_PROFILE_TEST_RESULT";
const HELPER_NAME: &str = "child_environment_tests::resource_workspace_profile_environment_helper";

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

#[test]
#[ignore = "仅由资源 Workspace 环境隔离测试作为子进程调用"]
fn resource_workspace_profile_environment_helper() {
    match std::env::var(HELPER_ROLE_ENV).as_deref() {
        Ok("full") => run_profile_probe(ResourceWorkspaceProfile::Full),
        Ok("targeted") => run_profile_probe(ResourceWorkspaceProfile::Targeted),
        Ok("probe") => {
            let value = std::env::var(PROFILE_ENV).unwrap_or_else(|_| "<missing>".to_owned());
            fs::write(
                std::env::var_os(RESULT_FILE_ENV).expect("应提供环境探针结果文件"),
                value,
            )
            .expect("应写入环境探针结果");
        }
        role => panic!("未知的资源 Workspace 环境测试角色：{role:?}"),
    }
}

#[test]
fn resource_workspace_profile_is_isolated_from_inherited_environment() {
    for (role, inherited, expected) in [
        ("full", "targeted", "<missing>"),
        ("targeted", "inherited-invalid", "targeted"),
    ] {
        let result_file = std::env::temp_dir().join(format!(
            "ryframe-resource-profile-{role}-{}.txt",
            std::process::id()
        ));
        let _ = fs::remove_file(&result_file);
        let output = Command::new(std::env::current_exe().expect("应能定位测试程序"))
            .args([HELPER_NAME, "--exact", "--ignored", "--nocapture"])
            .env(HELPER_ROLE_ENV, role)
            .env(RESULT_FILE_ENV, &result_file)
            .env(PROFILE_ENV, inherited)
            .output()
            .expect("应能启动资源 Workspace 环境辅助进程");
        assert!(
            output.status.success(),
            "{role} 环境辅助进程失败：stdout={}；stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(&result_file).expect("应读取环境探针结果"),
            expected,
            "{role} 必须先清除继承值，再按自身 profile 设置"
        );
        fs::remove_file(result_file).expect("应清理环境探针结果文件");
    }
}

fn run_profile_probe(profile: ResourceWorkspaceProfile) {
    let current_exe = std::env::current_exe().expect("应能定位测试程序");
    let executable = current_exe.to_str().expect("测试程序路径必须是有效 UTF-8");
    let mut owned_environment = resource_workspace_environment_for_profile(
        Path::new("workspace/frontend"),
        Path::new("target/ci/resource"),
        "1",
        profile,
    );
    owned_environment.push((HELPER_ROLE_ENV, "probe".to_owned()));
    let environment = owned_environment
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    run_with_env_removed(
        Path::new("."),
        executable,
        &[HELPER_NAME, "--exact", "--ignored", "--nocapture"],
        &environment,
        &[PROFILE_ENV],
    )
    .expect("应运行隔离后的环境探针");
}
