use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::{backend::allow_report, workspace::CandidateWorkspace};
use crate::business::generate_business_frontend;

pub(super) fn generate(
    backend: &Path,
    frontend: &CandidateWorkspace,
    frontend_source: &Path,
    target: &Path,
    catalog_target: &Path,
) -> Result<BTreeSet<PathBuf>, String> {
    println!("→ 从候选后端导出 OpenAPI 并生成 typed caller");
    let mut command = Command::new("cargo");
    command
        .args(["run", "--locked", "--offline", "--manifest-path"])
        .arg(backend.join("xtask/Cargo.toml"))
        .arg("--target-dir")
        .arg(target)
        .args(["--", "generate", "api", "--write", "--frontend-dir"])
        .arg(&frontend.path)
        .current_dir(backend)
        .env("CARGO_TARGET_DIR", target);
    run("候选契约生成", &mut command)?;
    println!("→ 生成业务列表、表单、路由和按钮权限");
    let report = generate_business_frontend(backend, &frontend.path, catalog_target)
        .map_err(|error| error.to_string())?;
    let mut allowed = api_artifacts(&frontend.source.root)?;
    allowed.extend(api_artifacts(&frontend.path)?);
    allow_report(&mut allowed, Path::new(""), &report);
    allowed.extend(
        [
            ".ryframe/generated.toml",
            "openapi/openapi.json",
            "openapi/candidate.json",
        ]
        .map(PathBuf::from),
    );
    validate_types(&frontend.path, frontend_source)?;
    Ok(allowed)
}

fn api_artifacts(root: &Path) -> Result<BTreeSet<PathBuf>, String> {
    let path = root.join("src/api/generated/ownership.json");
    let value: serde_json::Value = serde_json::from_slice(
        &fs::read(&path)
            .map_err(|error| format!("无法读取 API 产物清单 {}：{error}", path.display()))?,
    )
    .map_err(|error| error.to_string())?;
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err("API 产物清单版本无效；本批未安装".into());
    }
    let files = value
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or("API 产物清单缺少 files")?;
    files
        .iter()
        .map(|value| {
            let relative = value.as_str().ok_or("API 产物路径必须是字符串")?;
            if !relative.starts_with("src/api/generated/")
                && !matches!(
                    relative,
                    "src/shared/config/apiPrefix.generated.json"
                        | "src/shared/markdown/noticePolicy.generated.json"
                        | "src/shared/security/passwordPolicy.generated.json"
                )
            {
                return Err(format!("API 产物越界：{relative}；本批未安装"));
            }
            super::snapshot::safe_path(root, Path::new(relative))?;
            Ok(PathBuf::from(relative))
        })
        .collect()
}

fn validate_types(candidate: &Path, original: &Path) -> Result<(), String> {
    println!("→ 校验候选前端类型与生成页面");
    for (binary, args) in [
        (
            "node_modules/vue-tsc/bin/vue-tsc.js",
            vec!["--noEmit", "-p", "tsconfig.app.json"],
        ),
        (
            "node_modules/eslint/bin/eslint.js",
            vec!["src/generated/resources", "--max-warnings=0"],
        ),
    ] {
        let binary = tool(original, Path::new(binary))?;
        run(
            "候选前端校验",
            Command::new("node")
                .arg(binary)
                .args(args)
                .current_dir(candidate),
        )?;
    }
    Ok(())
}

pub(super) fn tool(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    root.ancestors().map(|ancestor| ancestor.join(relative)).find(|path| path.is_file())
        .ok_or_else(|| format!("缺少前端校验工具 {}；先在前端运行 corepack pnpm install --frozen-lockfile；本批未安装", root.join(relative).display()))
}

fn run(stage: &str, command: &mut Command) -> Result<(), String> {
    let status = command.status().map_err(|error| {
        format!("{stage}无法启动：{error}；核对 Cargo/Node、锁定依赖与配置路径；本批未安装")
    })?;
    if !status.success() {
        return Err(format!(
            "{stage}失败：{status}；实际原因见上方原始输出，修复编译、契约或类型错误后重新预览；本批未安装"
        ));
    }
    Ok(())
}
