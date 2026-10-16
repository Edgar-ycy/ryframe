//! 已移除身份能力的产品残留检查；历史记录与门禁断言不参与扫描。

use std::{
    collections::BTreeSet,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use crate::Result;

const REMOVED_SYMBOLS: &[&str] = &[
    "ServiceAccount",
    "ServiceDelegation",
    "ServiceCredential",
    "ServiceAccessAudit",
    "PepperKeyring",
    "AgentQuery",
    "AgentPageQuery",
    "AgentCapabilityResponse",
    "AgentUserResponse",
    "AgentDepartmentResponse",
    "AgentPostResponse",
    "AgentDictionaryItemResponse",
    "AgentDictionaryResponse",
    "PrebuiltApiEnvelope",
    "RyFrameApiKey",
    "X-RyFrame-Delegation",
    "/api/v1/agent/",
    "Agent API",
    "Agent 查询",
    "服务账号",
    "服务委托",
    "服务访问审计",
    "个人服务委托",
    "服务身份",
];

const REMOVED_PREFIXES: &[&str] = &[
    "service_account",
    "service-account",
    "service_delegation",
    "service-delegation",
    "service_credential",
    "service-credential",
    "service_access_audit",
    "service_access-audit",
    "service-access_audit",
    "service-access-audit",
    "agent_query",
    "agent-query",
    "agent_capabilit",
    "agent-capabilit",
    "agent_directory",
    "agent-directory",
    "agent_limiter",
    "agent-limiter",
];

const REMOVED_PATHS: &[&str] = &[
    "/crates/ryframe-application/src/agent/",
    "/crates/ryframe-application/src/ports/service_accounts/",
    "/crates/ryframe-application/src/system/service_account/",
    "/crates/ryframe-application/src/service_identity_secret.rs",
    "/crates/ryframe-api/src/dto/agent_dto.rs",
    "/crates/ryframe-api/src/handlers/agent_handler.rs",
    "/crates/ryframe-db/src/application_ports/agent/",
    "/crates/ryframe-db/src/application_ports/service_accounts/",
    "/crates/ryframe-db/src/repositories/service_authorization_repo.rs",
    "/src/api/generated/operations/agent.ts",
    "/src/api/generated/schema/agent.ts",
    "/src/features/service-accounts/",
    "/src/views/system/service-accounts/",
];

const SUFFIXES: &[&str] = &[
    "rs", "toml", "json", "sql", "yml", "yaml", "js", "cjs", "mjs", "ts", "cts", "mts", "vue",
];

/// 保持原门禁的大小写敏感子串匹配，分隔符仅在原规则允许的位置展开。
fn contains_removed(text: &str) -> bool {
    REMOVED_SYMBOLS
        .iter()
        .chain(REMOVED_PREFIXES)
        .any(|symbol| text.contains(*symbol))
}

pub(crate) fn check(root: &Path, frontend: Option<&Path>) -> Result<Vec<String>> {
    let frontend = frontend
        .map(|path| resolve_frontend(root, path))
        .transpose()?;
    let mut errors = scan_files(product_files(root)?)?;
    if let Some(frontend) = frontend {
        errors.extend(scan_files(frontend_files(&frontend)?)?);
    }
    Ok(errors)
}

fn resolve_frontend(root: &Path, frontend: &Path) -> Result<PathBuf> {
    let path = if frontend.is_absolute() {
        frontend.to_owned()
    } else {
        root.join(frontend)
    };
    let path = std::path::absolute(&path).map_err(|error| {
        format!(
            "无法解析服务身份检查使用的前端目录 {}：{error}",
            path.display()
        )
    })?;
    if !path.join("src").is_dir() {
        return Err(format!("--frontend-dir 必须指向现有前端项目：{}", path.display()).into());
    }
    Ok(path)
}

fn product_files(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut files = BTreeSet::from([
        root.join("Cargo.toml"),
        root.join("openapi").join("openapi.json"),
        root.join("sql").join("ryframe_config.sql"),
    ]);
    for entry in entries(&root.join("crates"))? {
        if !entry.path().is_dir() {
            continue;
        }
        let directory = entry.path();
        for name in ["src", "build_support"] {
            collect_files(&directory.join(name), false, &mut files)?;
        }
        for entry in entries(&directory)? {
            if entry.file_name() == "Cargo.toml" || entry.file_name() == "build.rs" {
                files.insert(entry.path());
            }
        }
    }
    for name in ["config", "catalog", "deploy"] {
        collect_files(&root.join(name), false, &mut files)?;
    }
    Ok(files)
}

fn frontend_files(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut files = BTreeSet::from([
        root.join("package.json"),
        root.join("openapi").join("openapi.json"),
        root.join("vite.config.ts"),
    ]);
    for entry in entries(root)? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("playwright") && name.ends_with(".ts") {
            files.insert(entry.path());
        }
    }
    collect_files(&root.join("src"), false, &mut files)?;
    collect_files(&root.join("scripts"), true, &mut files)?;
    Ok(files)
}

fn entries(directory: &Path) -> Result<Vec<fs::DirEntry>> {
    match fs::read_dir(directory) {
        Ok(entries) => entries
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|error| format!("无法遍历产品目录 {}：{error}", directory.display()).into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("无法读取产品目录 {}：{error}", directory.display()).into()),
    }
}

fn collect_files(
    directory: &Path,
    exclude_tests: bool,
    files: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in entries(directory)? {
        if exclude_tests && entry.file_name() == "tests" {
            continue;
        }
        let path = entry.path();
        // 与原递归扫描一致，不跟随目录符号链接；文件链接仍须检查内容。
        if entry.file_type()?.is_dir() {
            collect_files(&path, exclude_tests, files)?;
        } else if path.is_file() {
            files.insert(path);
        }
    }
    Ok(())
}

fn scan_files(files: BTreeSet<PathBuf>) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    for path in files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let normalized = format!(
            "/{}",
            path.to_string_lossy()
                .replace('\\', "/")
                .trim_start_matches('/')
        );
        if contains_removed(&name)
            || REMOVED_PATHS
                .iter()
                .any(|marker| normalized.contains(*marker))
        {
            errors.push(format!("已移除的产品文件仍存在：{}", path.display()));
        }
        if !name.starts_with(".env")
            && !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|suffix| SUFFIXES.contains(&suffix))
        {
            continue;
        }
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("无法读取产品文件 {}：{error}", path.display()))?;
        let source = source
            .strip_prefix('\u{feff}')
            .unwrap_or(&source)
            .replace("\r\n", "\n");
        for (number, line) in source
            .split([
                '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}',
                '\u{2029}',
            ])
            .enumerate()
        {
            if contains_removed(line) {
                errors.push(format!(
                    "已移除的产品定义仍存在：{}:{}",
                    path.display(),
                    number + 1
                ));
            }
        }
    }
    Ok(errors)
}
