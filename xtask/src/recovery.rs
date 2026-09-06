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
    if operation == "fresh-target" {
        if rest.iter().any(|value| value == "--backend-dir") {
            return Err(
                "fresh-target 由 cargo xtask 固定当前后端目录，不接受 --backend-dir".into(),
            );
        }
        let backend = root_dir();
        let backend = backend
            .to_str()
            .ok_or("后端目录必须能表示为 UTF-8 命令参数")?;
        let mut forwarded = Vec::with_capacity(rest.len() + 3);
        forwarded.push(operation.to_owned());
        forwarded.extend(rest.iter().cloned());
        forwarded.push("--backend-dir".to_owned());
        forwarded.push(backend.to_owned());
        return Ok(("scripts/devex_clone.py", forwarded));
    }
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
