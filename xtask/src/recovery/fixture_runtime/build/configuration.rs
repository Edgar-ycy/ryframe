//! 保留既有旁置双配置目录、实例文本和文件字节处理合同。
use super::super::model::*;
use crate::{Result, ci::databases::files};
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(super) fn copy(root: &Path, output: &Path, source: &Path) -> Result<(PathBuf, PathBuf)> {
    let (id, guards) = source_files(root, source)?;
    let configuration = sidecar(output)?;
    files::private_directory(&configuration)
        .map_err(|_| "旁置配置目录必须为新目录；保留现场禁止覆盖")?;
    let api = configuration.join("config");
    let worker = configuration.join("worker-config");
    for (destination, id) in [(&api, id), (&worker, 2)] {
        files::private_directory(destination).map_err(|_| "无法创建受限角色配置目录")?;
        for guard in &guards {
            let source = path(&guard["descriptor"]["path"])?;
            publish(
                &destination.join(source.file_name().ok_or("配置缺少文件名")?),
                &role_bytes(guard, id)?,
            )?;
            unchanged(guard)?;
        }
    }
    for guard in &guards {
        unchanged(guard)?;
    }
    Ok((api, worker))
}

pub(super) fn verify(root: &Path, output: &Path, source: &Path) -> Result<(PathBuf, PathBuf)> {
    let (id, guards) = source_files(root, source)?;
    let configuration = sidecar(output)?;
    directory(&configuration)?;
    let api = configuration.join("config");
    let worker = configuration.join("worker-config");
    for (destination, id) in [(&api, id), (&worker, 2)] {
        directory(destination)?;
        for guard in &guards {
            let source = path(&guard["descriptor"]["path"])?;
            let target = destination.join(source.file_name().ok_or("配置缺少文件名")?);
            safe(&target)?;
            check(
                fs::read(&target)? == role_bytes(guard, id)?,
                "暂停构建的角色配置与原 bootstrap 配置不符；保留现场，不能重新生成或覆盖",
            )?;
            unchanged(guard)?;
        }
    }
    Ok((api, worker))
}

fn source_files(root: &Path, source: &Path) -> Result<(u64, [serde_json::Value; 2])> {
    directory(source)?;
    let table = crate::rate_limit::load_app_table(root, Some(source), "test")
        .map_err(|_| "夹具来源配置无效；检查所选配置字段，不回显正文")?;
    let id = table
        .get("snowflake_worker_id")
        .and_then(toml::Value::as_integer)
        .filter(|v| (0..=1023).contains(v) && *v != 2)
        .ok_or("API 实例编号无效或与 Worker 2 冲突")?;
    Ok((
        id as u64,
        [
            snapshot(&source.join("app.toml"), MAX_BYTES)?,
            snapshot(&source.join("app.test.toml"), MAX_BYTES)?,
        ],
    ))
}

fn sidecar(output: &Path) -> Result<PathBuf> {
    let name = output
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("运行目录名称无效")?;
    let configuration = output
        .parent()
        .ok_or("运行目录缺少父目录")?
        .join(format!("{name}-configuration"));
    safe(&configuration)?;
    Ok(configuration)
}

fn role_bytes(guard: &serde_json::Value, id: u64) -> Result<Vec<u8>> {
    unchanged(guard)?;
    let source = path(&guard["descriptor"]["path"])?;
    let bytes = fs::read(&source)?;
    let bytes = if source.file_name().and_then(|v| v.to_str()) == Some("app.test.toml") {
        let raw = std::str::from_utf8(&bytes)
            .map_err(|_| "夹具配置不是 UTF-8")?
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let value = files::dispatch("instance-text", json!({"text":raw,"worker_id":id}))
            .map_err(|_| "角色配置实例文本无效；不回显配置正文")?;
        let raw = text(&value["text"])?;
        if cfg!(windows) {
            raw.replace('\n', "\r\n").into_bytes()
        } else {
            raw.as_bytes().to_vec()
        }
    } else {
        bytes
    };
    unchanged(guard)?;
    Ok(bytes)
}

fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    safe(path)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
