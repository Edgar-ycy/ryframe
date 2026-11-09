//! 核对恢复后的完整已有数据。登录与注销须持有运行控制权。
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use serde_json::{Value, json};

use super::super::{REGISTRATION_GUARD, REGISTRATION_LOCK};
use super::{authorized, check, commands::same_path, live, path};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{
        clone::native::{json as strict, lifecycle::lock::ProcessGuard},
        fixture_control::portable_path,
    },
};

const MARKER: &str = "__restore-dataset-verify-existing";
const LIMIT: u64 = 16 * 1024;

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().is_none_or(|argument| argument != MARKER) {
        return None;
    }
    let result = (|| {
        check(arguments.next().is_none(), "已有数据核验只接受标准输入")?;
        let mut bytes = Vec::new();
        std::io::stdin()
            .lock()
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)?;
        check(bytes.len() as u64 <= LIMIT, "已有数据核验请求超过限制")?;
        let input = strict::parse_fragment(std::str::from_utf8(&bytes)?, "已有数据核验请求")?;
        let value = execute(input)?;
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer(&mut stdout, &value)?;
        stdout.write_all(b"\n")?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    Some(if result.is_ok() {
        0
    } else {
        eprintln!(
            "恢复已有数据核验失败。登录或注销可能已发生，请保留原输入、运行代次和失败现场，禁止自动重放。"
        );
        1
    })
}

fn execute(input: Value) -> Result<Value> {
    strict::exact(
        &input,
        &["format_version", "kind", "write", "authority"],
        "已有数据核验请求",
    )?;
    check(
        input["format_version"].as_u64() == Some(1)
            && input["kind"] == "ryframe-restore-dataset-verification"
            && input["write"].as_bool() == Some(true),
        "已有数据核验必须明确授权登录与注销",
    )?;
    let backend = portable_path(&std::env::current_dir()?.canonicalize()?);
    let directory = validate_absolute_path(
        &backend.join(".local-tests"),
        LocalTestPathKind::ExistingDirectory,
    )?;
    validate_absolute_path(
        &directory.join(REGISTRATION_GUARD),
        LocalTestPathKind::ExistingFile,
    )?;
    let guard = ProcessGuard::acquire_named(&directory, REGISTRATION_GUARD)?;
    let checkpoint = || {
        guard.check()?;
        require_no_owner(&directory)
    };
    checkpoint()?;
    let expected = &input["authority"];
    let arguments = json!({
        "backend_dir": backend,
        "runtime_receipt": expected["runtime"]["path"],
        "target_plan": expected["target_plan"]["path"]
    });
    let result = authorized(&arguments, |backend, inputs, verified| {
        require_authority(expected.clone(), &verified.authority)?;
        let lineage = inputs.bound(&verified.authority["dataset_lineage"])?;
        let reference = inputs.bound(&lineage.value["reference_plan"])?;
        checkpoint()?;
        live::verify(
            backend,
            inputs,
            &verified.authority,
            &verified.runtime,
            &reference.value,
            &lineage.value,
            checkpoint,
        )
    });
    checkpoint()?;
    result
}

fn require_no_owner(directory: &Path) -> Result<()> {
    match fs::symlink_metadata(directory.join(REGISTRATION_LOCK)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("恢复运行仍有已登记或未收尾控制器，拒绝开启核验会话".into()),
    }
}

fn require_authority(mut expected: Value, actual: &Value) -> Result<()> {
    for key in [
        "runtime",
        "target_plan",
        "source_generation",
        "dataset_lineage",
    ] {
        let previous = path(&expected[key]["path"])?;
        let current = path(&actual[key]["path"])?;
        check(
            same_path(&previous, &current)?,
            "已有数据核验文件路径已变化",
        )?;
        expected[key]["path"] = actual[key]["path"].clone();
    }
    check(
        same_path(
            &path(&expected["execution_backend"])?,
            &path(&actual["execution_backend"])?,
        )?,
        "已有数据核验执行后端已变化",
    )?;
    expected["execution_backend"] = actual["execution_backend"].clone();
    check(expected == *actual, "已有数据核验权威与浏览器冻结值不同")
}
