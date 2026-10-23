//! seed 登记的只读证据边界；失败阶段不能回退到历史成功。

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{json, lifecycle::state, resources::common};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "run_manifest",
    "post_copy",
    "post_verify",
    "identity_plan",
    "identity_state",
    "identity_tool",
    "api_process",
    "node",
];

/// 保存完整阶段前像与登记绑定；后续授权后必须再次复核。
pub(crate) struct Registration {
    pub(crate) value: Value,
    directory: PathBuf,
    state: Value,
    descriptor: Value,
    manifest: Value,
    result: Value,
}

impl Registration {
    pub(crate) fn load(backend: &Path, directory: &Path, cleanup: bool) -> Result<Self> {
        validate_local_test_path(directory, backend, LocalTestPathKind::ExistingDirectory)?;
        let state = state::load(directory, !cleanup)?;
        let record = latest(&state, "seed-runtime", &["register"], None, cleanup)?;
        let result = common::read_bound(backend, &record["result"], "seed 登记阶段结果")?;
        let path = directory.join("seed-runtime.json");
        let descriptor = common::binding(&path)?;
        if result.get("status").and_then(Value::as_str) != Some("seed_runtime_registered")
            || result.get("registration") != Some(&descriptor)
        {
            return Err("seed 登记未绑定已发布的原始文件".into());
        }
        let value = json::read(&path)?;
        json::exact(&value, FIELDS, "seed 登记")?;
        let manifest = state::manifest_binding(directory)?;
        if value["format_version"].as_u64() != Some(1)
            || value["kind"].as_str() != Some("devex-clone-seed-runtime")
            || value["run_manifest"] != manifest
        {
            return Err("seed 登记不属于当前 run".into());
        }
        let result = record["result"].clone();
        let registered = Self {
            value,
            directory: directory.to_path_buf(),
            state,
            descriptor,
            manifest,
            result,
        };
        registered.assert_unchanged(backend, cleanup)?;
        Ok(registered)
    }

    pub(crate) fn assert_unchanged(&self, backend: &Path, cleanup: bool) -> Result<()> {
        let result_path = common::bound_file(backend, &self.result, "seed 登记阶段结果")?;
        if state::load(&self.directory, !cleanup)? != self.state
            || common::binding(&self.directory.join("seed-runtime.json"))? != self.descriptor
            || state::manifest_binding(&self.directory)? != self.manifest
            || common::binding(&result_path)? != self.result
        {
            return Err("seed 登记、阶段结果或固定清单在读取期间变化".into());
        }
        Ok(())
    }
}

/// cleanup 只跳过无关的损坏结果，不能跳过实际消费的登记结果与原始登记。
pub(crate) fn latest<'a>(
    state: &'a Value,
    stage: &str,
    modes: &[&str],
    current: Option<u64>,
    cleanup: bool,
) -> Result<&'a Value> {
    let record = state::attempts(state)?
        .iter()
        .rfind(|item| {
            item["stage"].as_str() == Some(stage)
                && item["mode"]
                    .as_str()
                    .is_some_and(|mode| modes.contains(&mode))
                && item["number"].as_u64() != current
                && (!cleanup || item["status"].as_str() == Some("passed"))
        })
        .ok_or("最新必需阶段尚未通过外层来源与清理核验")?;
    if record["status"].as_str() != Some("passed") {
        return Err("最新必需阶段尚未通过外层来源与清理核验".into());
    }
    Ok(record)
}

#[cfg(test)]
#[path = "seed/tests.rs"]
mod tests;
