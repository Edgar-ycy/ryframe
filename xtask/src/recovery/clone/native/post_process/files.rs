//! 文件位置、字节和独占新建沿用原受管边界；不覆盖失败现场。
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::{
        json as document,
        lifecycle::{manifest, stage::StageRequest, state},
        resources::common,
        seed,
        target_verify::{self, guard::Inputs},
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub(super) const RECOVERED_IDENTITY_TOOL: &str =
    "devex-seed-identity-verification-tool-successor-recovery";

pub(in crate::recovery::clone::native) fn absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("未登记生产者或控制锁仍存在；保留现场，不自动接管".into()),
    }
}

pub(in crate::recovery::clone::native) fn bindings(
    backend: &Path,
    value: &Value,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    match value {
        Value::Object(map)
            if map.len() == 3
                && ["path", "bytes", "sha256"]
                    .iter()
                    .all(|k| map.contains_key(*k)) =>
        {
            paths.insert(common::bound_file(backend, value, "固定请求文件")?);
        }
        Value::Object(map)
            if map.len() == 2 && ["path", "sha256"].iter().all(|k| map.contains_key(*k)) =>
        {
            paths.insert(target_verify::input::request::external_file(value)?);
        }
        Value::Object(map) => {
            for (key, value) in map {
                if key == "execution_backend" && value.get("execution_source").is_some() {
                    paths.extend(
                        crate::recovery::fixture_control::source_pair::execution::target_protected_inputs(
                            backend, value,
                        )?,
                    );
                } else {
                    bindings(backend, value, paths)?;
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                bindings(backend, value, paths)?;
            }
        }
        _ => (),
    }
    Ok(())
}

/// 只用于已经过正式生产者解析并按原描述符回读的后继登记。
pub(super) fn seed_successor_bindings(
    backend: &Path,
    successor: &Value,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if successor["kind"] != RECOVERED_IDENTITY_TOOL {
        return bindings(backend, successor, paths);
    }
    recovery_proof_bindings(backend, &successor["recovered_producer"], paths)?;
    for (field, value) in document::object(successor, "后继身份工具登记")? {
        if field != "recovered_producer" {
            bindings(backend, value, paths)?;
        }
    }
    Ok(())
}

fn recovery_proof_bindings(
    backend: &Path,
    proof: &Value,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let proof = document::exact(
        proof,
        &[
            "attempt",
            "previous",
            "failure",
            "controller",
            "launch",
            "producer",
            "stderr",
            "launch_source",
            "recovery_result",
            "recovery_controller",
            "recovery_intent",
            "legacy_source",
            "build",
        ],
        "已收口身份生产者证明",
    )?;
    for (field, value) in proof {
        match field.as_str() {
            "launch_source" => {
                paths.insert(recovered_file(value)?);
            }
            "build" => recovery_build_bindings(backend, value, paths)?,
            _ => bindings(backend, value, paths)?,
        }
    }
    Ok(())
}

fn recovery_build_bindings(
    backend: &Path,
    build: &Value,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let build = document::exact(
        build,
        &[
            "request",
            "tool",
            "validation",
            "frozen",
            "source_inputs",
            "source_base",
            "archived_inputs",
        ],
        "已收口身份生产者冻结构建",
    )?;
    for field in ["tool", "validation", "frozen", "source_inputs"] {
        paths.insert(recovered_file(&build[field])?);
    }
    for descriptor in document::array(&build["archived_inputs"], "原冻结构建源码副本")? {
        paths.insert(recovered_file(descriptor)?);
    }
    for field in ["request", "source_base"] {
        bindings(backend, &build[field], paths)?;
    }
    Ok(())
}

fn recovered_file(descriptor: &Value) -> Result<PathBuf> {
    manifest::validate_binding_shape(descriptor, "已收口核验的固定证据")?;
    let path = PathBuf::from(document::text(&descriptor["path"], "已收口核验的证据路径")?);
    validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
    if common::binding(&path)? != *descriptor {
        return Err("已收口核验的固定证据与登记摘要不一致".into());
    }
    Ok(path)
}

pub(super) fn check_proof_files(
    proofs: &serde_json::Map<String, Value>,
    changed: &'static str,
) -> Result<()> {
    for (path, expected) in proofs {
        let actual = common::binding(Path::new(path))?;
        if actual["bytes"] != expected["bytes"] || actual["sha256"] != expected["sha256"] {
            return Err(changed.into());
        }
    }
    Ok(())
}

pub(super) fn stage_output(request: &StageRequest<'_>, number: u64) -> Result<PathBuf> {
    let base = request.run_dir.join(request.stage);
    if !base.try_exists()? {
        fs::create_dir(&base)?;
    }
    common::local_directory(request.backend, &base, "复制后证据根")?;
    let output = base.join(format!("attempt-{number:04}"));
    common::ensure_new_directory(request.backend, &output, "复制后只读核验目录")?;
    Ok(output)
}

pub(super) fn protected(
    request: &StageRequest<'_>,
    copy: &Path,
    post: &seed::history::Post,
    proof_files: &Value,
    target: &Value,
    source: &Value,
    manifest: &Value,
) -> Result<Inputs> {
    let backend = request.backend;
    let run = request.run_dir;
    let number = request.attempt.ok_or("固定输入缺少阶段序号")?;
    let initialized = common::bound_file(backend, &manifest["initialized"], "初始化后像")?;
    let mut paths = BTreeSet::new();
    for path in [
        run.join("manifest.json"),
        run.join("state.json"),
        run.join("run.lock/owner.json"),
        state::controller_path(run, number),
        copy.join("plan.json"),
        copy.join("input.json"),
        copy.join("session.json"),
        copy.join("result.json"),
        copy.join("ledger/head.json"),
    ] {
        paths.insert(common::common_file(backend, &path, "复制后固定文件")?);
    }
    for value in [&post.request, &post.descriptor, target, source, manifest] {
        bindings(backend, value, &mut paths)?;
    }
    let proof_files = document::object(proof_files, "原来源完整证明文件")?;
    for path in proof_files.keys() {
        paths.insert(common::common_file(
            backend,
            Path::new(path),
            "原来源证明文件",
        )?);
    }
    paths.extend(target_verify::tree::files(
        initialized.parent().ok_or("初始化目录缺失")?,
        &target_verify::tree::snapshot(
            backend,
            initialized.parent().ok_or("初始化目录缺失")?,
            true,
        )?,
    )?);
    for request in [&post.request, target, source] {
        for field in ["backend_build", "maintenance_build"] {
            if request[field].is_null() {
                continue;
            }
            let build = common::read_bound(backend, &request[field], "冻结构建")?;
            for artifact in document::object(&build["artifacts"], "冻结构建产物")?.values() {
                paths.insert(target_verify::input::request::external_file(
                    &json!({"path":artifact["executable"],"sha256":artifact["sha256"]}),
                )?);
            }
        }
    }
    if request.mode == "amend" {
        let tree = target_verify::tree::snapshot(backend, &post.runtime, true)?;
        paths.extend(
            target_verify::tree::files(&post.runtime, &tree)?
                .into_iter()
                .filter(|path| {
                    !matches!(
                        path.file_name().and_then(|v| v.to_str()),
                        Some(
                            "api-control.guard" | "worker-control.guard" | "runtime-control.guard"
                        )
                    )
                }),
        );
        let runtime = document::read(&post.runtime.join("runtime.json"))?;
        for field in ["api_config_dir", "worker_config_dir"] {
            let directory = common::local_directory(
                backend,
                Path::new(document::text(&runtime[field], "原私有角色配置目录")?),
                "原私有角色配置目录",
            )?;
            for name in ["app.toml", "app.test.toml"] {
                let path = directory.join(name);
                if path.try_exists()? {
                    paths.insert(common::common_file(backend, &path, "原私有角色配置")?);
                }
            }
        }
    }
    let held = Inputs::open(paths)?;
    check_proof_files(proof_files, "原来源证明在取得保护前字节发生变化")?;
    held.check()?;
    Ok(held)
}

pub(super) fn publish(backend: &Path, path: &Path, value: &Value) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let before = common::binding(&common::common_file(backend, path, "已有固定收据")?)?;
            if common::read_bound(backend, &before, "已有固定收据")? != *value {
                return Err("已有固定收据内容不同，拒绝覆盖或自动重放".into());
            }
            common::bound_file(backend, &before, "已有固定收据后像")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            document::write_new(path, value)?
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub(in crate::recovery::clone::native) fn combine<T>(
    primary: Result<T>,
    checked: Result<()>,
) -> Result<T> {
    match (primary, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(super::diagnostic::combined(first, second)),
    }
}

pub(super) fn same_directory(backend: &Path, recorded: &Value, selected: &Path) -> Result<bool> {
    let recorded = Path::new(document::text(recorded, "登记运行位置")?);
    common::local_directory(backend, recorded, "登记运行目录")?;
    common::local_directory(backend, selected, "当前运行目录")?;
    let position = fs::canonicalize(selected)?;
    let (device, inode) = common::device_inode_for(selected, &fs::symlink_metadata(selected)?)?;
    let expected = serde_json::json!({"path":selected,"device":device,"inode":inode});
    let same = fs::canonicalize(recorded)? == position;
    if same {
        common::directory_identity(
            backend,
            &serde_json::json!({"path":recorded,"device":device,"inode":inode}),
        )?;
    }
    common::directory_identity(backend, &expected)?;
    common::local_directory(backend, recorded, "登记运行目录后像")?;
    if fs::canonicalize(selected)? != position || (same && fs::canonicalize(recorded)? != position)
    {
        return Err("登记运行位置在核对期间漂移".into());
    }
    Ok(same)
}

pub(super) fn same_file(backend: &Path, recorded: &Value, selected: &Path) -> Result<bool> {
    let recorded = Path::new(document::text(recorded, "登记历史位置")?);
    common::common_file(backend, recorded, "登记历史文件")?;
    common::common_file(backend, selected, "当前历史文件")?;
    let frozen = target_verify::guard::Inputs::open([recorded.to_owned(), selected.to_owned()])?;
    let selected_identity = common::device_inode_for(selected, &fs::symlink_metadata(selected)?)?;
    let same = crate::local_test_path::same_local_test_file(backend, selected, recorded)?;
    if same
        && common::device_inode_for(recorded, &fs::symlink_metadata(recorded)?)?
            != selected_identity
    {
        return Err("登记历史位置相同但实际文件身份不同".into());
    }
    frozen.check()?;
    Ok(same)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_launch_bindings_only_hold_named_external_evidence() -> Result<()> {
        let root = tempfile::tempdir()?;
        let backend = root.path();
        let local = backend.join(".local-tests");
        let archive = backend.join("frozen");
        fs::create_dir(&local)?;
        fs::create_dir(&archive)?;
        let request = local.join("request.json");
        let source = backend.join("launch.rs");
        let tool = archive.join("xtask.fixture");
        let validation = archive.join("validation.json");
        let frozen = archive.join("frozen.json");
        let source_inputs = archive.join("source-inputs.json");
        let archived_input = archive.join("archived.rs");
        let expected = BTreeSet::from([
            request.clone(),
            source.clone(),
            tool.clone(),
            validation.clone(),
            frozen.clone(),
            source_inputs.clone(),
            archived_input.clone(),
        ]);
        for path in &expected {
            fs::write(path, b"fixture")?;
        }
        let local_binding = common::binding(&request)?;
        let source_binding = common::binding(&source)?;
        let mut record = json!({
            "kind": RECOVERED_IDENTITY_TOOL,
            "previous_recovery": local_binding,
            "recovered_producer": {
                "attempt": 70, "previous": local_binding, "failure": local_binding,
                "controller": local_binding, "launch": local_binding, "producer": local_binding,
                "stderr": local_binding, "launch_source": source_binding,
                "recovery_result": local_binding, "recovery_controller": local_binding,
                "recovery_intent": local_binding, "legacy_source": local_binding,
                "build": {
                    "request": local_binding, "tool": common::binding(&tool)?,
                    "validation": common::binding(&validation)?, "frozen": common::binding(&frozen)?,
                    "source_inputs": common::binding(&source_inputs)?, "source_base": "fixture",
                    "archived_inputs": [common::binding(&archived_input)?],
                },
            },
        });
        let mut paths = BTreeSet::new();
        seed_successor_bindings(backend, &record, &mut paths)?;
        assert_eq!(paths, expected);
        Inputs::open(paths)?.check()?;

        let mut ordinary = record.clone();
        ordinary["kind"] = json!("devex-seed-identity-verification-tool-successor");
        assert!(seed_successor_bindings(backend, &ordinary, &mut BTreeSet::new()).is_err());

        let mut changed = record.clone();
        changed["recovered_producer"]["build"]["archived_inputs"][0]["sha256"] =
            json!("a".repeat(64));
        assert!(seed_successor_bindings(backend, &changed, &mut BTreeSet::new()).is_err());

        let mut unexpected = record.clone();
        unexpected["recovered_producer"]["build"]["unexpected"] = json!(true);
        assert!(seed_successor_bindings(backend, &unexpected, &mut BTreeSet::new()).is_err());

        record["recovered_producer"]["build"]["request"] = source_binding;
        assert!(seed_successor_bindings(backend, &record, &mut BTreeSet::new()).is_err());
        Ok(())
    }
}
