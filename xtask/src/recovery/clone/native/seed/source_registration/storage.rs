//! 将当前target存储观察与不可变目标初始化绑定；不创建或重启存储。
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{
    Result,
    cli::CloneSide,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::{
        input::digest_shape,
        json as document,
        lifecycle::state,
        resources::{common, storage::registered},
        target_verify::{input::request_configuration::canonical_path, published},
    },
};

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/storage.rs"]
mod tests;

pub(crate) fn verify(backend: &Path, run: &Path, manifest: &Value) -> Result<Value> {
    common::local_directory(backend, run, "seed来源运行目录")?;
    let manifest_binding = state::manifest_binding(run)?;
    if common::read_bound(backend, &manifest_binding, "seed来源固定清单")? != *manifest {
        return Err("seed来源清单不同于当前run固定清单".into());
    }
    let before = state::load(run, true)?;
    let observed = registered::current(backend, run, CloneSide::Target)?
        .ok_or("seed源必须绑定本run实际重启的target RustFS")?;
    let initialized = common::bound_file(backend, &manifest["initialized"], "seed目标初始化")?;
    let (history, target) = published::unlocked_history(backend, &initialized)?;
    let target_binding = published::request_binding(backend, &target)?;
    let observed_exe = executable(&observed["storage"]["identity"]["executable"])?;
    let target_exe = executable(&target["storage"]["rustfs"]["identity"]["executable"])?;
    compare(&observed, &target, &observed_exe, &target_exe)?;
    for (field, label) in [
        ("request", "seed存储固定请求"),
        ("restart_result", "seed存储重启外层结果"),
    ] {
        common::bound_file(backend, &observed[field], label)?;
    }
    let (after_history, after_target) = published::unlocked_history(backend, &initialized)?;
    if after_history != history
        || after_target != target
        || published::request_binding(backend, &after_target)? != target_binding
    {
        return Err("seed存储核验期间目标初始化或请求绑定变化".into());
    }
    for (field, label) in [
        ("request", "seed存储固定请求"),
        ("restart_result", "seed存储重启外层结果"),
    ] {
        common::bound_file(backend, &observed[field], label)?;
    }
    common::bound_file(backend, &manifest["initialized"], "seed目标初始化")?;
    common::bound_file(backend, &manifest_binding, "seed来源固定清单")?;
    if state::load(run, true)? != before {
        return Err("seed存储核验期间完整阶段状态变化；保留现场，禁止重放".into());
    }
    Ok(observed)
}

fn executable(value: &Value) -> Result<PathBuf> {
    let path = Path::new(document::text(value, "RustFS可执行路径")?);
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)
        .map_err(|_| "RustFS可执行文件必须是普通文件且不能经过链接")?;
    canonical_path(path)
}

fn compare(observed: &Value, target: &Value, observed_exe: &Path, target_exe: &Path) -> Result<()> {
    let endpoint = document::text(&observed["api_url"], "已观察RustFS端点")?;
    let expected_endpoint = document::text(&target["target"]["s3"]["endpoint"], "目标RustFS端点")?;
    let sha = digest_shape(&observed["storage"]["sha256"])?;
    let expected_sha = digest_shape(&target["storage"]["rustfs"]["sha256"])?;
    if endpoint != expected_endpoint || sha != expected_sha || observed_exe != target_exe {
        return Err("seed源RustFS端点或二进制不属于旧run的target".into());
    }
    Ok(())
}
