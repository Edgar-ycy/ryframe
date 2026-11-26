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
    build_dir: &Path,
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
        // 子 xtask 用同一独占目录构建并运行 OpenAPI 导出程序。
        .env("CARGO_TARGET_DIR", target)
        .env("CARGO_BUILD_BUILD_DIR", build_dir);
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
            node_validation_command(&binary, candidate)?.args(args),
        )?;
    }
    Ok(())
}

fn node_validation_command(binary: &Path, candidate: &Path) -> Result<Command, String> {
    #[cfg(windows)]
    let (binary_path, candidate_path) = (windows_node_path(binary)?, windows_node_path(candidate)?);
    #[cfg(windows)]
    let (binary, candidate) = (binary_path.as_ref(), candidate_path.as_ref());

    let mut command = Command::new("node");
    command.arg(binary).current_dir(candidate);
    Ok(command)
}

#[cfg(windows)]
fn windows_node_path(path: &Path) -> Result<std::borrow::Cow<'_, Path>, String> {
    use std::borrow::Cow;

    let Some(ordinary) = windows_node_spelling(path)? else {
        return Ok(Cow::Borrowed(path));
    };
    // Vue 按同一种路径拼写计算相对类型引用，只在子进程边界改用已核对的普通路径。
    let same = fs::canonicalize(path)
        .and_then(|original| fs::canonicalize(&ordinary).map(|converted| original == converted))
        .map_err(|error| {
            format!(
                "候选前端校验无法核对 Node 的普通 Windows 路径 {}：{error}；请核对路径长度和访问权限；本批未安装",
                ordinary.display(),
            )
        })?;
    if !same {
        return Err(format!(
            "候选前端校验的 Node 路径转换前后指向不同位置：{}；本批未安装",
            path.display(),
        ));
    }
    Ok(Cow::Owned(ordinary))
}

#[cfg(windows)]
fn windows_node_spelling(path: &Path) -> Result<Option<PathBuf>, String> {
    use std::{
        ffi::OsString,
        path::{Component, Prefix},
    };

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Ok(None);
    };
    let mut ordinary = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:", char::from(drive))),
        Prefix::VerbatimUNC(server, share) => {
            let mut root = OsString::from(r"\\");
            root.push(server);
            root.push(r"\");
            root.push(share);
            PathBuf::from(root)
        }
        Prefix::Disk(_) | Prefix::UNC(..) => return Ok(None),
        Prefix::Verbatim(_) | Prefix::DeviceNS(_) => {
            return Err(format!(
                "候选前端校验不支持此 Node 路径的 Windows 命名空间：{}；本批未安装",
                path.display(),
            ));
        }
    };
    ordinary.extend(components.map(|component| component.as_os_str()));
    if !ordinary.is_absolute() {
        return Err(format!(
            "候选前端校验无法生成普通 Windows 绝对路径：{}；本批未安装",
            path.display(),
        ));
    }
    Ok(Some(ordinary))
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

#[cfg(all(test, windows))]
mod tests {
    use std::path::{Component, Prefix};

    use super::*;

    #[test]
    fn node_validation_uses_ordinary_paths_for_the_same_file_and_directory() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests");
        fs::create_dir_all(&root).unwrap();
        let fixture = tempfile::tempdir_in(root).unwrap();
        let candidate = fixture.path().join("候选 with spaces");
        fs::create_dir(&candidate).unwrap();
        let entry = candidate.join("工具 with spaces.js");
        fs::write(&entry, b"").unwrap();
        let canonical_entry = fs::canonicalize(entry).unwrap();
        let canonical_candidate = fs::canonicalize(candidate).unwrap();

        let command = node_validation_command(&canonical_entry, &canonical_candidate).unwrap();
        let binary = Path::new(command.get_args().next().unwrap());
        let directory = command.get_current_dir().unwrap();
        for (ordinary, canonical) in [(binary, canonical_entry), (directory, canonical_candidate)] {
            assert!(matches!(
                canonical.components().next(),
                Some(Component::Prefix(prefix))
                    if matches!(prefix.kind(), Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(..))
            ));
            assert!(matches!(
                ordinary.components().next(),
                Some(Component::Prefix(prefix))
                    if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::UNC(..))
            ));
            assert_eq!(fs::canonicalize(ordinary).unwrap(), canonical);
        }
    }

    #[test]
    fn node_validation_preserves_unc_server_and_share_and_rejects_other_namespaces() {
        let ordinary = Path::new(r"\\server\share\候选 with spaces");
        assert_eq!(
            windows_node_spelling(Path::new(r"\\?\UNC\server\share\候选 with spaces"))
                .unwrap()
                .as_deref(),
            Some(ordinary),
        );
        assert!(windows_node_spelling(ordinary).unwrap().is_none());
        assert!(windows_node_spelling(Path::new(r"\\?\Volume{fixture}\tool.js")).is_err());
    }
}
