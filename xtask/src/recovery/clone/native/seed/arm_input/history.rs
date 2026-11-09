//! 只认领由真实阶段账本和精确文件前缀证明的 ARM 输出。
use std::{
    collections::BTreeSet,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{MODE, check_owner, model};
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as document,
        lifecycle::{manifest, state},
        resources::common,
    },
};

pub(in crate::recovery) struct Active<'a> {
    pub(in crate::recovery) run: &'a Path,
    number: u64,
    input: &'a Value,
    pub(in crate::recovery) request: &'a Value,
}

impl<'a> Active<'a> {
    pub(super) fn new(run: &'a Path, number: u64, input: &'a Value, request: &'a Value) -> Self {
        Self {
            run,
            number,
            input,
            request,
        }
    }

    fn verify(&self, backend: &Path, state: &Value) -> Result<()> {
        check_owner(self.run, state, self.number)?;
        if common::read_bound(backend, self.input, "ARM 失败续接的当前请求")? != *self.request
        {
            return Err("ARM 失败续接请求变化".into());
        }
        model::request(self.request)?;
        Ok(())
    }
}

pub(super) struct Published {
    pub(super) side: String,
    output: PathBuf,
    attempt: u64,
    request: Value,
}

pub(super) fn published(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    request: &Value,
) -> Result<Vec<Published>> {
    let results = collect(backend, run, rows, &request["source_registration"])?;
    for previous in &results {
        if model::SOURCE
            .iter()
            .any(|field| previous.request[*field] != request[*field])
        {
            return Err("两侧 arm 必须共享同一已发布来源、重绑定、导出及代次".into());
        }
    }
    Ok(results)
}

fn collect(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    descriptor: &Value,
) -> Result<Vec<Published>> {
    let mut results = Vec::new();
    let mut sides = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    for row in rows.iter().filter(|row| is_arm(row, "passed")) {
        let result = read_passed(backend, run, rows, row, descriptor)?;
        if !sides.insert(result.side.clone()) || !outputs.insert(result.output.clone()) {
            return Err("同一 seed 源出现重复 ARM 侧或输出目录".into());
        }
        results.push(result);
    }
    if let Some(first) = results.first()
        && results.iter().any(|previous| {
            model::SOURCE
                .iter()
                .any(|field| previous.request[*field] != first.request[*field])
        })
    {
        return Err("历史 ARM 结果混入其他来源、导出或代次".into());
    }
    Ok(results)
}

fn read_passed(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    row: &Value,
    source: &Value,
) -> Result<Published> {
    let attempt = number(row)?;
    let result_path = common::bound_file(backend, &row["result"], "历史 ARM 阶段结果")?;
    if result_path != state::result_path(run, attempt) {
        return Err("历史 ARM 结果不属于固定阶段".into());
    }
    let result = common::read_bound(backend, &row["result"], "历史 ARM 阶段结果")?;
    let request_path = common::bound_file(backend, &result["request"], "历史 ARM 请求")?;
    let manifest_path = common::bound_file(backend, &result["manifest"], "历史 ARM 清单")?;
    let output = request_path.parent().ok_or("历史 ARM 请求缺少目录")?;
    if request_path != output.join("request.json")
        || manifest_path != output.join("manifest.json")
        || prefix(backend, output)? != Some(2)
    {
        return Err("历史 ARM 已发布结果没有引用本 run 的完整前缀".into());
    }
    if output != output_path(run, attempt) {
        let failed = rows
            .iter()
            .find(|original| {
                is_arm(original, "failed")
                    && original["number"]
                        .as_u64()
                        .is_some_and(|n| n < attempt && output == output_path(run, n))
            })
            .ok_or("历史 ARM 采用没有引用同 run 的原失败阶段")?;
        if !failed["result"].is_null()
            && common::read_bound(backend, &failed["result"], "原失败 ARM 的残留结果")? != result
        {
            return Err("历史 ARM 已采用结果与原失败阶段残留不同".into());
        }
    }
    let request = common::read_bound(backend, &result["request"], "历史 ARM 请求")?;
    let manifest = common::read_bound(backend, &result["manifest"], "历史 ARM 清单")?;
    model::check_result(&result, &request)?;
    check_manifest(backend, &request, &manifest)?;
    check_source(backend, rows, source, &request, &result, &manifest)?;
    let target = common::read_bound(
        backend,
        &request["target_registration"],
        "历史 ARM 目标登记",
    )?;
    let target = common::read_bound(backend, &target["request"], "历史 ARM 目标请求")?;
    let side = document::text(&result["target_side"], "历史 ARM 目标侧")?;
    if target["side"] != side {
        return Err("历史 ARM 结果侧别不属于原目标请求".into());
    }
    common::bound_file(backend, &row["result"], "历史 ARM 结果")?;
    Ok(Published {
        side: side.to_owned(),
        output: output.to_owned(),
        attempt,
        request,
    })
}

fn check_manifest(backend: &Path, request: &Value, value: &Value) -> Result<()> {
    manifest::validate(value, backend)?;
    model::check_fields(value, request)?;
    let copy = Path::new(document::text(
        &request["copy_directory"],
        "历史 ARM 复制路径",
    )?);
    if value["id"] != request["id"]
        || value["build_bridges"] != request["build_bridges"]
        || Path::new(document::text(
            &value["copy_directory"],
            "历史 ARM 清单复制路径",
        )?) != copy
        || value["copy_stage"] != "seed_to_arm"
    {
        return Err("ARM 前缀清单与原请求不同".into());
    }
    Ok(())
}

fn check_source(
    backend: &Path,
    rows: &[Value],
    source: &Value,
    request: &Value,
    result: &Value,
    manifest: &Value,
) -> Result<()> {
    if request["source_registration"] != *source
        || result["source_request"] != manifest["source_request"]
        || result["target_storage_run"] != manifest["target_storage_run"]
    {
        return Err("历史 ARM 结果混入其他来源或目标代次".into());
    }
    let registered = common::read_bound(backend, source, "历史 ARM 原来源")?;
    if registered["status"] != "seed_source_registered"
        || registered["remote_writes"].as_u64() != Some(0)
        || registered["outbox_drained"] != true
        || registered["restore_qualified"] != false
        || result["source_storage"] != registered["source_storage"]
    {
        return Err("历史 ARM 结果未绑定原来源存储收据".into());
    }
    if model::request(request)? {
        let export = phase(
            backend,
            rows,
            &request["source_export_result"],
            &["source-export", "source-export-reconcile"],
        )?;
        for field in [
            "source_registration",
            "source_rebind",
            "review_successor",
            "source_generation",
        ] {
            if export[field] != request[field] {
                return Err("历史 ARM 与已发布共享导出来源不同".into());
            }
        }
        if export["export"] != request["source_export"]
            || export["source_request"] != result["source_request"]
        {
            return Err("历史 ARM 与已发布共享导出证据不同".into());
        }
    }
    let generation = if request.get("source_generation").is_some() {
        phase(
            backend,
            rows,
            &request["source_generation"],
            &["source-generation-stop"],
        )?
    } else if registered["source_request"] == result["source_request"] {
        registered
    } else {
        let path = common::bound_file(backend, source, "历史 ARM 源结果")?;
        let run = path
            .parent()
            .and_then(Path::parent)
            .ok_or("历史 ARM 来源缺少原账本")?;
        ordinary_generation(backend, run, &result["source_request"])?
    };
    if result["generation_verified"] != generation["generation_verified"]
        || result["source_request"] != generation["source_request"]
    {
        return Err("历史 ARM 源请求或代次证明变化".into());
    }
    common::bound_file(
        backend,
        &result["generation_verified"],
        "历史 ARM 原代次证明",
    )?;
    Ok(())
}

fn ordinary_generation(backend: &Path, run: &Path, request: &Value) -> Result<Value> {
    let saved = state::load(run, true)?;
    let mut values = Vec::new();
    for row in state::attempts(&saved)?.iter().filter(|row| {
        row["stage"] == "seed-runtime"
            && row["mode"] == "source-generation-stop"
            && row["status"] == "passed"
    }) {
        let value = common::read_bound(backend, &row["result"], "普通 ARM 原代次")?;
        if value["source_request"] == *request {
            values.push(value);
        }
    }
    match values.as_slice() {
        [value] => Ok(value.clone()),
        _ => Err("普通 ARM 没有绑定唯一已发布来源代次".into()),
    }
}

fn phase(backend: &Path, rows: &[Value], binding: &Value, modes: &[&str]) -> Result<Value> {
    let matching = rows
        .iter()
        .filter(|row| {
            row["stage"] == "seed-runtime"
                && row["status"] == "passed"
                && row["result"] == *binding
                && row["mode"]
                    .as_str()
                    .is_some_and(|mode| modes.contains(&mode))
        })
        .count();
    if matching != 1 {
        return Err("历史 ARM 没有引用唯一正式来源阶段".into());
    }
    common::read_bound(backend, binding, "历史 ARM 来源阶段")
}

/// 在源历史读取中只证明本地失败前缀，不授予其他阶段写入权限。
pub(in crate::recovery) fn prove(
    backend: &Path,
    run: &Path,
    saved: &Value,
    failed: &Value,
    source: &Value,
    active: Option<&Active<'_>>,
) -> Result<()> {
    if !is_arm(failed, "failed") {
        return Err("ARM 失败证明未绑定原阶段".into());
    }
    let rows = state::attempts(saved)?;
    if !rows.contains(failed) {
        return Err("ARM 失败记录不属于当前原账本".into());
    }
    let passed = collect(backend, run, rows, source)?;
    let number = number(failed)?;
    let path = output_path(run, number);
    if passed.iter().any(|arm| arm.output == path) {
        return Ok(());
    }
    if let Some(active) = active
        && active.run == run
        && active.request["source_registration"] == *source
        && active.number > number
    {
        active.verify(backend, saved)?;
        pending(backend, run, rows, &passed, active.request)?;
        return Ok(());
    }
    if failed["result"].is_null()
        && prefix(backend, &path)?.is_none()
        && passed.iter().any(|arm| arm.attempt > number)
    {
        return Ok(());
    }
    Err("seed 历史包含未受控的 ARM 失败前缀".into())
}

pub(super) fn pending(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    adopted: &[Published],
    request: &Value,
) -> Result<Option<PathBuf>> {
    let mut found = None;
    for failed in rows.iter().filter(|row| is_arm(row, "failed")) {
        let path = output_path(run, number(failed)?);
        if adopted.iter().any(|arm| arm.output == path) {
            continue;
        }
        let Some(size) = prefix(backend, &path)? else {
            if !failed["result"].is_null() {
                return Err("ARM 失败阶段已有结果却缺少原输出".into());
            }
            continue;
        };
        if size > 0 && document::read(&path.join("request.json"))? != *request {
            return Err("arm-input 已有局部请求不同，须保留现场核对".into());
        }
        if size == 2 {
            check_manifest(
                backend,
                request,
                &document::read(&path.join("manifest.json"))?,
            )?;
        }
        if !failed["result"].is_null() {
            let saved = common::read_bound(backend, &failed["result"], "ARM 失败阶段残留结果")?;
            model::check_result(&saved, request)?;
            if size != 2
                || saved["request"] != common::binding(&path.join("request.json"))?
                || saved["manifest"] != common::binding(&path.join("manifest.json"))?
            {
                return Err("ARM 失败结果与原固定前缀不同".into());
            }
        }
        if found.replace(path).is_some() {
            return Err("多个历史 arm-input 目录可能含交接，不能猜测".into());
        }
    }
    Ok(found)
}

pub(super) fn write_prefix(
    backend: &Path,
    run: &Path,
    attempt: u64,
    pending: Option<PathBuf>,
    request: &Value,
    manifest: &Value,
) -> Result<PathBuf> {
    let output = if let Some(output) = pending {
        output
    } else {
        common::local_directory(backend, &run.join("seed-runtime"), "ARM 输出父目录")?;
        let output = output_path(run, attempt);
        fs::create_dir(&output)?;
        output
    };
    let size = prefix(backend, &output)?.ok_or("ARM 输出目录缺失")?;
    for (index, (name, value)) in [("request.json", request), ("manifest.json", manifest)]
        .iter()
        .enumerate()
    {
        let path = output.join(name);
        if index < size {
            if document::read(&path)? != **value {
                return Err("arm-input 已有局部证据不同，须保留现场核对".into());
            }
        } else {
            document::write_new(&path, value)?;
        }
    }
    if prefix(backend, &output)? != Some(2) {
        return Err("ARM 输出在发布期间变化".into());
    }
    Ok(output)
}

fn prefix(backend: &Path, directory: &Path) -> Result<Option<usize>> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.is_dir() || digest::linked(&metadata) => {
            return Err("历史 arm-input 输出不是无链接普通文件目录".into());
        }
        Ok(_) => {}
    }
    let directory = common::local_directory(backend, directory, "ARM 证据目录")?;
    let mut files = BTreeSet::new();
    for item in fs::read_dir(&directory)? {
        let item = item?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| "ARM 证据文件名无效")?;
        common::common_file(backend, &item.path(), "ARM 证据文件")?;
        files.insert(name);
    }
    let valid = match files.len() {
        0 => true,
        1 => files.contains("request.json"),
        2 => files.contains("request.json") && files.contains("manifest.json"),
        _ => false,
    };
    if !valid {
        return Err("历史 arm-input 含未知或非前缀局部证据".into());
    }
    Ok(Some(files.len()))
}

fn output_path(run: &Path, number: u64) -> PathBuf {
    run.join(format!("seed-runtime/attempt-{number:04}"))
}

fn number(row: &Value) -> Result<u64> {
    row["number"]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| "ARM 阶段序号无效".into())
}

fn is_arm(row: &Value, status: &str) -> bool {
    row["stage"] == "seed-runtime" && row["mode"] == MODE && row["status"] == status
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/arm_input_tests.rs"]
mod tests;
