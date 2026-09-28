use std::{fs, path::Path};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::{
        CacheOptions, CloneCommand, CloneRuntimeOptions, PostCopyOptions, SeedRuntimeOptions,
        StorageOptions,
    },
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

pub(crate) fn protocol(command: &CloneCommand, root: &Path) -> Result<String> {
    if matches!(command, CloneCommand::Help) {
        return Err("clone 帮助不启动私有阶段程序".into());
    }
    let payload = serde_json::to_string(&json!({
        "format_version": 1,
        "kind": "ryframe-xtask-recovery-clone",
        "request": {
            "backend_dir": text(root, "后端目录")?,
            "command": command.name(),
            "effect": command.effect().as_str(),
            "options": options(command, root)?,
            "write": command.effect().requires_write(),
        },
    }))?;
    if payload.len() > 64 * 1024 || payload.contains(['\r', '\n', '\0']) {
        return Err("clone 私有协议过长或包含换行符/NUL".into());
    }
    Ok(payload)
}

fn options(command: &CloneCommand, root: &Path) -> Result<Value> {
    Ok(match command {
        CloneCommand::Help => return Err("clone 帮助没有私有请求参数".into()),
        CloneCommand::Plan { input, output } => json!({
            "input": local(input, root, LocalTestPathKind::ExistingFile, "复制输入")?,
            "output": output_file(output, root, "复制计划输出")?,
        }),
        CloneCommand::Verify { plan } => json!({
            "plan": local(plan, root, LocalTestPathKind::ExistingFile, "复制计划")?,
        }),
        CloneCommand::Init { manifest, run_dir } => json!({
            "manifest": local(manifest, root, LocalTestPathKind::ExistingFile, "复制清单")?,
            "run_dir": local(run_dir, root, LocalTestPathKind::NewDirectory, "复制运行目录")?,
        }),
        CloneCommand::Status { run_dir } => json!({ "run_dir": run(run_dir, root)? }),
        CloneCommand::Stage { run_dir, stage } => json!({
            "run_dir": run(run_dir, root)?, "stage": stage.as_str(), "mode": stage.mode().as_str(),
        }),
        CloneCommand::Runtime(value) => runtime(value, root)?,
        CloneCommand::Recover {
            run_dir,
            owner_binding,
        }
        | CloneCommand::RecoverCopy {
            run_dir,
            owner_binding,
        } => json!({
            "run_dir": run(run_dir, root)?,
            "owner_binding": local(owner_binding, root, LocalTestPathKind::ExistingFile, "控制器绑定")?,
        }),
        CloneCommand::Bridge {
            build,
            inventory,
            output,
        } => json!({
            "build": local(build, root, LocalTestPathKind::ExistingFile, "构建收据")?,
            "inventory": local(inventory, root, LocalTestPathKind::ExistingFile, "来源清单")?,
            "output": output_file(output, root, "来源桥接输出")?,
        }),
        CloneCommand::PostCopy(value) => post_copy(value, root)?,
        CloneCommand::SeedRuntime(value) => seed_runtime(value, root)?,
        CloneCommand::Storage(value) => storage(value, root)?,
        CloneCommand::Cache(value) => cache(value, root)?,
        CloneCommand::MaintenanceBuild { output } => json!({
            "operation": "build",
            "output": local(output, root, LocalTestPathKind::NewDirectory, "维护构建目录")?,
        }),
        CloneCommand::MaintenanceVerify { output } => json!({
            "operation": "verify", "output": existing(output, root, "维护构建收据")?,
        }),
    })
}

fn runtime(value: &CloneRuntimeOptions, root: &Path) -> Result<Value> {
    Ok(json!({
        "run_dir": run(&value.run_dir, root)?,
        "side": value.side.as_str(),
        "operation": value.operation.as_str(),
        "roles": value.roles.iter().map(|role| role.as_str()).collect::<Vec<_>>(),
    }))
}

fn post_copy(value: &PostCopyOptions, root: &Path) -> Result<Value> {
    Ok(json!({
        "run_dir": run(&value.run_dir, root)?,
        "operation": value.operation.as_str(),
        "request": optional_file(value.request.as_deref(), root, "post-copy 请求")?,
        "producer_binding": optional_file(value.producer_binding.as_deref(), root, "post-copy 生产者绑定")?,
    }))
}

fn seed_runtime(value: &SeedRuntimeOptions, root: &Path) -> Result<Value> {
    Ok(json!({
        "run_dir": run(&value.run_dir, root)?,
        "operation": value.operation.as_str(),
        "request": optional_file(value.request.as_deref(), root, "seed 请求")?,
        "producer_binding": optional_file(value.producer_binding.as_deref(), root, "seed 生产者绑定")?,
    }))
}

fn storage(value: &StorageOptions, root: &Path) -> Result<Value> {
    Ok(json!({
        "run_dir": run(&value.run_dir, root)?, "side": value.side.as_str(),
        "operation": value.operation.as_str(),
        "request": optional_file(value.request.as_deref(), root, "存储重启请求")?,
    }))
}

fn cache(value: &CacheOptions, root: &Path) -> Result<Value> {
    Ok(json!({
        "run_dir": run(&value.run_dir, root)?, "operation": value.operation.as_str(),
        "request": optional_file(value.request.as_deref(), root, "缓存重启请求")?,
    }))
}

fn run(value: &Path, root: &Path) -> Result<String> {
    local(
        value,
        root,
        LocalTestPathKind::ExistingDirectory,
        "复制运行目录",
    )
}

fn optional_file(value: Option<&Path>, root: &Path, label: &str) -> Result<Option<String>> {
    value
        .map(|path| local(path, root, LocalTestPathKind::ExistingFile, label))
        .transpose()
}

fn output_file(value: &Path, root: &Path, label: &str) -> Result<String> {
    let result = local(value, root, LocalTestPathKind::OutputFile, label)?;
    if !value.parent().is_some_and(Path::is_dir) {
        return Err(format!("{label}的父目录必须已经存在").into());
    }
    Ok(result)
}

fn existing(value: &Path, root: &Path, label: &str) -> Result<String> {
    let metadata = fs::symlink_metadata(value)?;
    let kind = if metadata.is_file() {
        LocalTestPathKind::ExistingFile
    } else if metadata.is_dir() {
        LocalTestPathKind::ExistingDirectory
    } else {
        return Err(format!("{label}必须是普通文件或目录").into());
    };
    local(value, root, kind, label)
}

fn local(value: &Path, root: &Path, kind: LocalTestPathKind, label: &str) -> Result<String> {
    validate_local_test_path(value, root, kind).map_err(|error| format!("{label}无效：{error}"))?;
    text(value, label)
}

fn text(value: &Path, label: &str) -> Result<String> {
    let text = value
        .to_str()
        .ok_or_else(|| format!("{label}必须能表示为 UTF-8"))?;
    if text.is_empty() || text.contains(['\r', '\n', '\0']) {
        return Err(format!("{label}不能为空或包含换行符/NUL").into());
    }
    Ok(text.to_owned())
}
