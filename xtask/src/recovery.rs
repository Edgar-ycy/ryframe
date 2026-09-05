use std::path::Path;

use crate::{Result, process::run_owned, workspace::root_dir};

pub(crate) fn run(arguments: &[String], frontend_dir: &Path) -> Result<()> {
    let (script, forwarded) = recovery_command(arguments, frontend_dir)?;
    let mut command = Vec::with_capacity(forwarded.len() + 1);
    command.push(script.to_owned());
    command.extend(forwarded);
    run_owned(&root_dir(), "python", &command)
}

pub(crate) fn recovery_command(
    arguments: &[String],
    frontend_dir: &Path,
) -> Result<(&'static str, Vec<String>)> {
    let (operation, rest) = arguments.split_first().ok_or("恢复验收缺少明确阶段")?;
    if operation != "runtime" {
        return Ok(("scripts/restore_reference.py", arguments.to_vec()));
    }
    let mut forwarded = rest.to_vec();
    if matches!(rest.first().map(String::as_str), Some("bind" | "verify")) {
        let frontend = frontend_dir
            .to_str()
            .ok_or("前端目录必须能表示为 UTF-8 命令参数")?;
        forwarded.push("--frontend-dir".to_owned());
        forwarded.push(frontend.to_owned());
    }
    Ok(("scripts/restore_runtime.py", forwarded))
}
