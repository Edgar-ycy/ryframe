use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::error::{GateError, GateResult};

pub(crate) const CGROUP_ROOT: &str = "/sys/fs/cgroup";
pub(crate) const TEST_CASE: &str =
    "devex_memory_tests::linux_cgroup_joins_before_exec_covers_grandchildren_and_cleans_its_group";
pub(crate) const ENVIRONMENT_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "CARGO_INCREMENTAL",
    "CARGO_TERM_COLOR",
    "CARGO_NET_RETRY",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CgroupPlan {
    pub(crate) format_version: u32,
    pub(crate) run_id: String,
    pub(crate) attempt: String,
    pub(crate) nonce: String,
    pub(crate) source_sha: String,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) root: PathBuf,
    pub(crate) backend: PathBuf,
    pub(crate) cargo: PathBuf,
    pub(crate) environment: BTreeMap<String, String>,
}

impl CgroupPlan {
    pub(crate) fn root_for(
        cgroup_root: &Path,
        run_id: &str,
        attempt: &str,
        nonce: &str,
    ) -> PathBuf {
        cgroup_root.join(format!("ryframe-devex-ci-{run_id}-{attempt}-{nonce}"))
    }
}

pub(crate) fn validate_plan_at(
    plan: &CgroupPlan,
    expected_backend: &Path,
    cgroup_root: &Path,
) -> GateResult<()> {
    if plan.format_version != 1 {
        return Err(plan_error("format_version 必须为 1"));
    }
    if !positive_decimal(&plan.run_id) || !positive_decimal(&plan.attempt) {
        return Err(plan_error(
            "run_id 与 attempt 必须是 1 到 20 位的正十进制整数",
        ));
    }
    if !lower_hex(&plan.nonce, 32) {
        return Err(plan_error("nonce 必须是 32 位小写十六进制字符串"));
    }
    if !lower_hex(&plan.source_sha, 40) {
        return Err(plan_error("source_sha 必须是 40 位小写 Git SHA"));
    }
    if plan.uid == 0 || plan.gid == 0 {
        return Err(plan_error("Cargo 必须使用原 runner 的非 root uid/gid"));
    }
    let expected_root = CgroupPlan::root_for(cgroup_root, &plan.run_id, &plan.attempt, &plan.nonce);
    if plan.root != expected_root {
        return Err(plan_error(format!(
            "cgroup 根路径必须是本次 CI 的精确路径 {}，实际为 {}",
            expected_root.display(),
            plan.root.display()
        )));
    }
    if plan.backend != expected_backend {
        return Err(plan_error(format!(
            "计划后端目录 {} 与当前目录 {} 不一致",
            plan.backend.display(),
            expected_backend.display()
        )));
    }
    if !plan.cargo.is_absolute() {
        return Err(plan_error(format!(
            "Cargo 路径必须是绝对路径，实际为 {}",
            plan.cargo.display()
        )));
    }
    if let Some(name) = plan
        .environment
        .keys()
        .find(|name| !ENVIRONMENT_ALLOWLIST.contains(&name.as_str()))
    {
        return Err(plan_error(format!(
            "环境变量 {name} 未在 cgroup 门禁白名单中，拒绝把潜在凭据写入证据"
        )));
    }
    Ok(())
}

pub(crate) fn validate_test_output(code: i32, output: &str) -> GateResult<()> {
    let header = Regex::new(&format!(r"(?m)^test {} \.\.\.", regex::escape(TEST_CASE))).map_err(
        |error| GateError::new("test", format!("无法编译 cgroup 测试输出匹配器：{error}")),
    )?;
    let result = Regex::new(r"(?m)^test result: .+$")
        .map_err(|error| GateError::new("test", format!("无法编译测试结果匹配器：{error}")))?;
    let headers = header.find_iter(output).count();
    let final_result = result
        .find_iter(output)
        .filter_map(|value| value.as_str().strip_prefix("test result: "))
        .last()
        .unwrap_or_default();
    if code != 0 || headers != 1 || !final_result.starts_with("ok. 1 passed; 0 failed; 0 ignored;")
    {
        return Err(GateError::new(
            "test",
            format!(
                "cgroup 实机用例未恰好通过一次；零测试、ignored 或失败均不算验收（退出码 {code}，用例输出行 {headers}，结果 `{final_result}`）"
            ),
        ));
    }
    Ok(())
}

pub(crate) fn cargo_command(plan: &CgroupPlan) -> Vec<String> {
    vec![
        plan.cargo.to_string_lossy().into_owned(),
        "test".to_owned(),
        "--locked".to_owned(),
        "-p".to_owned(),
        "xtask".to_owned(),
        "--no-default-features".to_owned(),
        "--test".to_owned(),
        "internal".to_owned(),
        "--target-dir".to_owned(),
        plan.backend
            .join("target/ci/devex-cgroup")
            .to_string_lossy()
            .into_owned(),
        "--jobs".to_owned(),
        "2".to_owned(),
        "--".to_owned(),
        "--exact".to_owned(),
        TEST_CASE.to_owned(),
        "--ignored".to_owned(),
        "--nocapture".to_owned(),
        "--test-threads=1".to_owned(),
    ]
}

pub(crate) fn privileged_identity_matches(
    plan: &CgroupPlan,
    sudo_uid: &str,
    sudo_gid: &str,
) -> bool {
    sudo_uid == plan.uid.to_string() && sudo_gid == plan.gid.to_string()
}

pub(crate) fn plan_error(message: impl Into<String>) -> GateError {
    GateError::new("plan", message)
}

fn positive_decimal(value: &str) -> bool {
    value.len() <= 20
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_digit() && (index != 0 || byte != b'0'))
        && !value.is_empty()
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
