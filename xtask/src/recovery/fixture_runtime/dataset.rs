//! 正式参考数据工厂；完整运行权威与保护始终由当前 Rust 调用持有。
use super::{model::*, runtime::Runtime};
use crate::{Result, recovery::clone::native::target_verify::guard::Inputs};
use serde_json::{Value, json};
use std::{fs, path::Path};

#[path = "dataset/execute.rs"]
mod execute;
#[path = "dataset/plan.rs"]
mod plan;
const TIMEOUT_SECONDS: u64 = 43_200;

pub(super) fn run(backend: &Path, operation: &str, args: &Value) -> Result<Value> {
    check(
        matches!(operation, "plan" | "prepare"),
        "参考数据工厂操作无效",
    )?;
    let environment = path(&args["environment"])?;
    let output = plan::canonical(&path(&args["runtime"])?)?;
    let runtime = Runtime::load(backend, &environment, &output)?;
    let protected = runtime.protect()?;
    let selected = if operation == "plan" {
        &args["output"]
    } else {
        &args["plan"]
    };
    let file = plan::child(&runtime, &path(selected)?, operation == "plan", false)?;
    let saved = if operation == "prepare" {
        Some(document(&file)?)
    } else {
        None
    };
    let work = if let Some((value, _)) = &saved {
        path(&value["work_dir"])?
    } else {
        path(&args["work_dir"])?
    };
    check(work != file, "参考数据工作目录与计划输出必须不同")?;
    let side = text(&args["side"])?;
    let authority = plan::build(backend, &environment, &runtime, &work, side)?;
    let check_inputs = || -> Result<()> {
        for input in &protected {
            input.check()?;
        }
        authority.verify()?;
        runtime.verify()
    };
    check_inputs()?;
    if let Some((saved, guard)) = saved {
        check(
            saved["target_side"] == side,
            "参考数据计划与显式恢复目标侧不一致",
        )?;
        check(
            saved == authority.value,
            "参考数据计划与当前夹具来源、工具或运行时不一致",
        )?;
        let held = Inputs::open([file.clone()])?;
        unchanged(&guard)?;
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("参考数据计划文件名无效")?;
        let reports = output.join(format!("{stem}.native-reports"));
        let failure = output.join(format!("{stem}.prepare-failed.json"));
        safe(&reports)?;
        check(
            !reports.try_exists()? && !failure.try_exists()?,
            "参考数据准备已有证据，禁止覆盖或重放",
        )?;
        fs::create_dir(&reports)?;
        let verify = || {
            held.check()?;
            unchanged(&guard)?;
            check_inputs()
        };
        let result = execute::run(&runtime, &authority, &file, &reports, &failure, verify)?;
        return Ok(result);
    }
    write_new(&file, &authority.value)?;
    let published = snapshot(&file, MAX_BYTES)?;
    check_inputs()?;
    unchanged(&published)?;
    Ok(json!({"status":"reference_fixture_dataset_planned",
        "plan_sha256":crate::recovery::clone::native::digest::plan_hash(&authority.value)?,"remote_writes":0}))
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/fixture_runtime/dataset.rs"]
mod tests;
