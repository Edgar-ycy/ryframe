use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};

use super::environment::ProcessEnvironment;

pub(crate) fn install_frontend_dependencies(
    frontend: &Path,
    corepack: &Path,
    environment: &ProcessEnvironment,
) -> Result<(), String> {
    let fingerprint = frontend_dependency_fingerprint(frontend)?;
    let marker = frontend
        .join("node_modules")
        .join(".ryframe-resource-replay-dependencies");
    if fs::read_to_string(&marker)
        .ok()
        .is_some_and(|value| value.trim() == fingerprint)
    {
        return Ok(());
    }
    let output = Command::new(corepack)
        .args(["pnpm", "install", "--offline", "--frozen-lockfile"])
        .current_dir(frontend)
        .env_clear()
        .envs(environment)
        .output()
        .map_err(|error| format!("前端 replay 依赖安装无法启动：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "前端 replay 依赖离线安装失败；请先预热 pnpm store：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if !frontend.join("node_modules").is_dir() {
        return Err("前端 replay 离线安装后仍缺少 node_modules".to_owned());
    }
    fs::write(&marker, format!("{fingerprint}\n"))
        .map_err(|error| format!("无法写入前端 replay 依赖指纹：{error}"))
}

pub(crate) fn frontend_dependency_fingerprint(frontend: &Path) -> Result<String, String> {
    let mut files = [
        "package.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        ".npmrc",
    ]
    .into_iter()
    .map(|name| frontend.join(name))
    .filter(|path| path.is_file())
    .collect::<Vec<_>>();
    let patches = frontend.join("patches");
    if patches.is_dir() {
        collect_files(&patches, &mut files)?;
    }
    files.sort_by_key(|path| {
        path.strip_prefix(frontend)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    });
    let mut digest = Sha256::new();
    for path in files {
        let relative = path
            .strip_prefix(frontend)
            .map_err(|_| format!("前端依赖文件逃逸仓库：{}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/")
            .into_bytes();
        let body = fs::read(&path)
            .map_err(|error| format!("无法读取前端依赖文件 {}：{error}", path.display()))?;
        digest.update((relative.len() as u32).to_be_bytes());
        digest.update(&relative);
        digest.update((body.len() as u64).to_be_bytes());
        digest.update(&body);
    }
    Ok(format!("sha256:{}", hex(&digest.finalize())))
}

fn collect_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("无法读取前端 patches 目录 {}：{error}", directory.display()))?
    {
        let entry = entry.map_err(|error| format!("无法读取前端 patches 项：{error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, output)?;
        } else if path.is_file() {
            output.push(path);
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}
