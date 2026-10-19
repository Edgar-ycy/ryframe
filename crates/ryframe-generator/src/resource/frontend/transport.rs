use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use super::{ResourceError, failure};

#[cfg(windows)]
const COREPACK: &str = "corepack.cmd";
#[cfg(not(windows))]
const COREPACK: &str = "corepack";

pub(super) fn invoke(frontend: &Path, input: &[u8]) -> Result<Vec<u8>, ResourceError> {
    let frontend = std::path::absolute(frontend)
        .map_err(|error| failure(format!("无法解析前端路径：{error}；本批未安装")))?;
    let script = frontend.join("scripts/resource-render/cli.mjs");
    if !script.is_file() || !frontend.join("package.json").is_file() {
        return Err(
            failure("前端资源渲染入口或 package.json 不存在；本批未安装")
                .with_file(script.to_string_lossy()),
        );
    }
    let runtime = runtime_root(&frontend)?;
    let mut child = Command::new(COREPACK)
        .args(["pnpm", "--config.verifyDepsBeforeRun=error", "exec", "node"])
        .arg(&script)
        .current_dir(runtime)
        .env("COREPACK_ENABLE_NETWORK", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| failure(format!("前端资源渲染进程无法启动：{error}；本批未安装")))?;
    let written = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("没有取得渲染进程标准输入"))
        .and_then(|mut stdin| stdin.write_all(input));
    // 请求失败仍须等待并回收本次进程，保留退出状态及渲染器诊断。
    let output = child
        .wait_with_output()
        .map_err(|error| failure(format!("无法等待前端资源渲染结果：{error}；本批未安装")))?;
    if !output.status.success() || written.is_err() {
        let reason = String::from_utf8_lossy(&output.stderr);
        let reason = reason.trim();
        let input_error = written
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        return Err(failure(format!(
            "前端资源渲染失败：{}；标准输入：{}；渲染器诊断：{}；本批未安装",
            output.status,
            if input_error.is_empty() {
                "已完整传递"
            } else {
                &input_error
            },
            if reason.is_empty() {
                "无标准错误输出"
            } else {
                reason
            },
        )));
    }
    if !output.stderr.is_empty() {
        return Err(failure(format!(
            "前端渲染进程成功退出但包含标准错误输出：{}；本批未安装",
            String::from_utf8_lossy(&output.stderr).trim(),
        )));
    }
    Ok(output.stdout)
}

// 候选副本没有 node_modules；复用匹配的已安装环境，禁止 pnpm 在副本隐式安装。
fn runtime_root(frontend: &Path) -> Result<PathBuf, ResourceError> {
    let package = fs::read(frontend.join("package.json"))
        .map_err(|error| failure(format!("无法读取前端 package.json：{error}")))?;
    for root in frontend.ancestors() {
        if !root.join("node_modules/.modules.yaml").is_file() {
            continue;
        }
        let installed = fs::read(root.join("package.json"))
            .map_err(|error| failure(format!("无法读取已安装前端环境：{error}")))?;
        if installed != package {
            return Err(failure(
                "候选前端 package.json 与已安装环境不一致；本批未安装",
            ));
        }
        return Ok(root.to_path_buf());
    }
    Err(failure("没有找到匹配且已安装依赖的前端环境；本批未安装"))
}

#[cfg(test)]
#[path = "../../../tests/internal/frontend_runtime.rs"]
mod tests;
