//! 已审计启动前失败只作历史事实读取；不执行旧代码，不把缺目录当作零写入证明。
use super::super::builds;
use crate::recovery::source::evidence::{inventory, staging};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{digest, json as document, lifecycle::state, resources::common},
};
use serde_json::{Value, json};
use sha2::Digest;
use std::{collections::BTreeMap, path::Path};

#[path = "prelaunch/binary.rs"]
mod binary;
#[path = "prelaunch/closed.rs"]
mod closed;
#[path = "prelaunch/data.rs"]
mod data;
pub(in crate::recovery::source::evidence) use closed::closed;
pub(super) use closed::starts;

const START: &str = "source-generation-start";
const RECOVER: &str = "source-generation-recover";

pub(in crate::recovery::source::evidence) fn proof(
    backend: &Path,
    run: &Path,
    start: &Value,
) -> Result<Value> {
    failed(start, START, "ValueError")?;
    let number = number(start)?;
    missing(backend, &run.join(format!("g{number:04}")))?;
    let source = &start["sources"];
    audited(backend, source, data::AUDITED_TREE)?;
    let (controller, owner) = read_controller(run, start)?;
    let failure = failure(backend, run, start, &controller, data::Kind::Start)?;
    absent_controller(&owner)?;
    if read_controller(run, start)? != (controller.clone(), owner) {
        return Err("原启动控制器在历史证明复核期间变化".into());
    }
    common::bound_file(backend, &failure, "原启动失败")?;
    missing(backend, &run.join(format!("g{number:04}")))?;
    Ok(
        json!({"start":digest::plan_hash(start)?,"failure":failure,"controller":controller,
        "source":source,"tree":data::AUDITED_TREE}),
    )
}

pub(super) fn reregistration(
    backend: &Path,
    run: &Path,
    attempt: &Value,
    previous: &Value,
) -> Result<()> {
    failed(attempt, START, "ValueError")?;
    if number(attempt)? != number(previous)?.checked_add(1).ok_or("历史阶段序号溢出")?
        || previous["stage"] != "cache-target"
        || previous["mode"] != "restart"
        || previous["status"] != "passed"
        || !previous["error_type"].is_null()
        || previous["result"].is_null()
        || attempt["sources"] != previous["sources"]
    {
        return Err("生成重试只能继承同源相邻且零输出的原收据重登记失败".into());
    }
    audited(backend, &attempt["sources"], data::REREGISTRATION_TREE)?;
    let (controller, owner) = read_controller(run, attempt)?;
    let failure = failure(
        backend,
        run,
        attempt,
        &controller,
        data::Kind::Reregistration,
    )?;
    let number = number(attempt)?;
    for path in [
        run.join(format!("g{number:04}")),
        run.join(format!("results/{number:04}.json")),
        run.join(format!("seed-runtime/attempt-{number:04}")),
    ] {
        missing(backend, &path)?;
    }
    let tool = crate::recovery::fixture_control::portable_path(&std::env::current_exe()?);
    crate::recovery::clone::native::seed::source_registration::quiet::require_stopped(
        backend,
        run,
        &owner["identity"],
        &tool,
    )?;
    if read_controller(run, attempt)? != (controller, owner) {
        return Err("历史收据重登记失败的控制器在复核期间变化".into());
    }
    common::bound_file(backend, &failure, "收据重登记原失败")?;
    audited(backend, &attempt["sources"], data::REREGISTRATION_TREE)?;
    for path in [
        run.join(format!("g{number:04}")),
        run.join(format!("results/{number:04}.json")),
        run.join(format!("seed-runtime/attempt-{number:04}")),
    ] {
        missing(backend, &path)?;
    }
    Ok(())
}

pub(in crate::recovery::source::evidence) fn origin(
    backend: &Path,
    run: &Path,
    prefix: &[Value],
) -> Result<(Value, Value)> {
    let starts = prefix
        .iter()
        .enumerate()
        .filter(|(_, row)| row["stage"] == "seed-runtime" && row["mode"] == START)
        .collect::<Vec<_>>();
    let [(index, start)] = starts.as_slice() else {
        return Err("未启动恢复必须继承唯一原 START".into());
    };
    let tail = &prefix[*index..];
    if tail.len() == 1 {
        proof(backend, run, start)?;
        return Ok(((*start).clone(), Value::Null));
    }
    for (offset, row) in tail.iter().enumerate().skip(1) {
        failed(row, RECOVER, "CalledProcessError")?;
        if number(row)?
            != number(start)?
                .checked_add(u64::try_from(offset)?)
                .ok_or("历史序号溢出")?
        {
            return Err("原未启动采集失败不是连续相邻阶段".into());
        }
    }
    let collection = match tail {
        [_, first] => collection(backend, run, start, first, None)?,
        [_, first, second] => collection(backend, run, start, second, Some(first))?,
        _ => return Err("未启动恢复包含重复采集或其他历史阶段".into()),
    };
    Ok(((*start).clone(), collection))
}

fn collection(
    backend: &Path,
    run: &Path,
    start: &Value,
    record: &Value,
    previous: Option<&Value>,
) -> Result<Value> {
    let original = proof(backend, run, start)?;
    let prior = previous
        .map(|previous| collection(backend, run, start, previous, None))
        .transpose()?
        .unwrap_or(Value::Null);
    let observed = previous.is_some();
    failed(record, RECOVER, "CalledProcessError")?;
    if number(record)?
        != number(start)?
            .checked_add(if observed { 2 } else { 1 })
            .ok_or("历史采集序号溢出")?
    {
        return Err("原采集失败不属于唯一已审计相邻阶段".into());
    }
    let expected_tree = if observed {
        data::OBSERVATION_TREE
    } else {
        data::COLLECTION_TREE
    };
    audited(backend, &record["sources"], expected_tree)?;
    let number = number(record)?;
    missing(backend, &run.join(format!("g{number:04}")))?;
    let (controller, owner) = read_controller(run, record)?;
    let failure = failure(
        backend,
        run,
        record,
        &controller,
        if observed {
            data::Kind::Observation
        } else {
            data::Kind::Collection
        },
    )?;
    let output = common::local_directory(
        backend,
        &run.join(format!("seed-runtime/attempt-{number:04}")),
        "原失败采集目录",
    )?;
    let files = inventory::manifest(&output)?;
    collection_files(&files, observed)?;
    let (maintenance, artifact) = binary::verify(backend, &output, &record["sources"], observed)?;
    absent_controller(&owner)?;
    let path = Path::new(document::text(&artifact["executable"], "已审计只读二进制")?);
    let artifact_binding = common::binding(path)?;
    let repeated_prior = previous
        .map(|old| collection(backend, run, start, old, None))
        .transpose()?
        .unwrap_or(Value::Null);
    if read_controller(run, record)? != (controller.clone(), owner)
        || inventory::manifest(&output)? != files
        || proof(backend, run, start)? != original
        || repeated_prior != prior
    {
        return Err("原失败采集证明在核对期间变化".into());
    }
    common::bound_file(backend, &failure, "原失败采集")?;
    common::bound_file(backend, &maintenance, "原只读维护构建")?;
    common::bound_file(backend, &artifact_binding, "原只读二进制")?;
    missing(backend, &run.join(format!("g{number:04}")))?;
    let mut value = json!({"attempt":digest::plan_hash(record)?,"original_start":original,"failure":failure,
        "controller":controller,"source":record["sources"],"tree":expected_tree,"previous":prior,
        "files":files,"maintenance":maintenance});
    value[if observed { "tenant-data" } else { "migrate" }] = artifact_binding;
    Ok(value)
}

fn collection_files(files: &[Value], observed: bool) -> Result<()> {
    let actual = files
        .iter()
        .map(|row| {
            Ok((
                document::text(&row["path"], "历史文件路径")?,
                document::text(&row["sha256"], "历史文件摘要")?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    if if observed {
        digest::plan_hash(&json!(files))? != data::OBSERVATION_FILES
    } else {
        actual != data::COLLECTION_FILES.iter().copied().collect()
    } {
        return Err("原失败采集不属于完整已审计文件集合".into());
    }
    Ok(())
}

fn read_controller(run: &Path, attempt: &Value) -> Result<(Value, Value)> {
    let mut running = attempt.clone();
    running["status"] = json!("running");
    running["finished_at"] = Value::Null;
    running["error_type"] = Value::Null;
    let (controller, owner) = state::controller_record(run, number(attempt)?, &running)?;
    document::exact(
        &owner,
        &["format_version", "identity", "directory", "manifest_sha256"],
        "历史控制器所有权",
    )?;
    common::validate_process_identity(&owner["identity"])?;
    if owner["format_version"].as_u64() != Some(1)
        || owner["directory"] != json!(run)
        || owner["manifest_sha256"] != common::binding(&run.join("manifest.json"))?["sha256"]
    {
        return Err("历史控制器不属于原固定运行清单".into());
    }
    Ok((controller, owner))
}

fn failure(
    backend: &Path,
    run: &Path,
    attempt: &Value,
    controller: &Value,
    kind: data::Kind,
) -> Result<Value> {
    let number = number(attempt)?;
    let path = run.join(format!("failure-{number:04}.json"));
    let binding = common::binding(&path)?;
    let value = common::read_bound(backend, &binding, "原已审计失败")?;
    let expected = json!({"format_version":1,"kind":"devex-stage-failure","attempt":number,"stage":"seed-runtime",
        "mode":attempt["mode"],"error_type":attempt["error_type"],"frames":data::frames(kind),"controller":controller});
    if value != expected {
        return Err("原失败调用链不是已审计的固定零写入边界".into());
    }
    Ok(binding)
}

fn audited(backend: &Path, source: &Value, tree: &str) -> Result<()> {
    staging::validate_source(source)?;
    if source["snapshot"]["files"] != json!([])
        || source["snapshot"]["patch_sha256"] != hex::encode(sha2::Sha256::digest(b""))
        || builds::git_tree(
            backend,
            document::text(&source["snapshot"]["head"], "历史源码 SHA")?,
        )? != tree
    {
        return Err("历史失败不属于精确干净的已审计源码树".into());
    }
    Ok(())
}

fn absent_controller(owner: &Value) -> Result<()> {
    let pid = owner["identity"]["pid"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or("历史控制器 PID 无效")?;
    if common::process_identity(pid)?.is_some() {
        return Err("原失败控制器仍存活或 PID 已复用".into());
    }
    Ok(())
}

fn failed(row: &Value, mode: &str, error: &str) -> Result<()> {
    if row["stage"] != "seed-runtime"
        || row["mode"] != mode
        || row["status"] != "failed"
        || !row["result"].is_null()
        || row["error_type"] != error
    {
        return Err("历史失败阶段、状态或错误类型不属于已审计分支".into());
    }
    Ok(())
}

fn missing(backend: &Path, path: &Path) -> Result<()> {
    validate_local_test_path(path, backend, LocalTestPathKind::OutputFile)?;
    if path.try_exists()? {
        return Err("原零输出失败存在未登记 generation、结果或 attempt 目录".into());
    }
    Ok(())
}

fn number(value: &Value) -> Result<u64> {
    value["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| "历史阶段序号无效".into())
}
