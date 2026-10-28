//! 原生 Linux 工具绑定复用观察器配置；写许可由独立入口和先行收据提供。
use super::super::{digest, json as doc};
use crate::{Result, frontend_origin::manifest};
use ryframe_config::{LinuxObserverSettings, ToolSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(target_os = "linux")]
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn wsl_program() -> Result<PathBuf> {
    let path = std::env::var_os("PATH").ok_or_else(wsl_entry_error)?;
    wsl_from_path(&path)
}

fn wsl_entry_error() -> Box<dyn std::error::Error> {
    super::diagnostic::failure(
        super::diagnostic::Category::WslEntryRejected,
        super::diagnostic::Outcome::NotStarted,
    )
}

fn wsl_from_path(path: &std::ffi::OsStr) -> Result<PathBuf> {
    let name = if cfg!(windows) { "wsl.exe" } else { "wsl" };
    for directory in std::env::split_paths(path).filter(|path| path.is_absolute()) {
        let candidate = directory.join(name);
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) => {
                let path = super::model::plain(&candidate).map_err(|_| wsl_entry_error())?;
                if !metadata.is_file() {
                    return Err(wsl_entry_error());
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(wsl_entry_error()),
        }
    }
    Err(wsl_entry_error())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeTool {
    pub distribution: String,
    pub user: String,
    pub uid: u32,
    pub program: PathBuf,
    pub linux_program: String,
    pub source_root: PathBuf,
    pub linux_source_root: String,
    pub source_commit: String,
    pub sha256: String,
    pub source_sha256: String,
    pub sources: Value,
}

impl NativeTool {
    pub(super) fn capture(backend: &Path) -> Result<Self> {
        crate::recovery::runtime::dataset_authority::reads::tool_settings(backend)?;
        let settings = ToolSettings::load(backend)?
            .verification
            .linux_observer
            .ok_or("缓存原生控制需要 verification.linux_observer 的显式来源与 ELF 绑定")?;
        super::linux_observer::validate_settings(&settings)?;
        manifest::git_source(&settings.source_root, &settings.source_commit)?;
        let sources = manifest::capture(&settings.source_root)?;
        if manifest::sha256(&sources)? != settings.source_sha256 {
            return Err("缓存原生工具完整来源摘要不符".into());
        }
        Ok(Self::from_settings(settings, sources))
    }

    fn from_settings(s: LinuxObserverSettings, sources: Value) -> Self {
        Self {
            distribution: s.distribution,
            user: s.user,
            uid: s.uid,
            program: s.program,
            linux_program: s.linux_program,
            source_root: s.source_root,
            linux_source_root: s.linux_source_root,
            source_commit: s.source_commit,
            sha256: s.sha256,
            source_sha256: s.source_sha256,
            sources,
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        super::model::linux_path(self.program.to_str().ok_or("工具 Windows 路径不是UTF-8")?)?;
        super::model::linux_path(
            self.source_root
                .to_str()
                .ok_or("来源 Windows 路径不是UTF-8")?,
        )?;
        let safe_name = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        };
        let linux = |s: &str| {
            s.starts_with('/')
                && s.len() <= 4096
                && !s.contains(['\\', '\0', '\r', '\n'])
                && !s.split('/').skip(1).any(|p| matches!(p, "" | "." | ".."))
        };
        if !safe_name(&self.distribution)
            || !safe_name(&self.user)
            || !linux(&self.linux_program)
            || !linux(&self.linux_source_root)
            || !manifest::hex_digest(&self.source_commit, 40)
            || !manifest::hex_digest(&self.sha256, 64)
            || !manifest::hex_digest(&self.source_sha256, 64)
            || manifest::sha256(&self.sources)? != self.source_sha256
        {
            return Err("缓存原生工具绑定无效".into());
        }
        Ok(())
    }

    pub(super) fn finish(&self, backend: &Path) -> Result<()> {
        use super::diagnostic::{Category, Outcome, failure};
        let current = Self::capture(backend)
            .map_err(|_| failure(Category::ToolConfigurationRejected, Outcome::Unknown))?;
        if current != *self {
            return Err(failure(Category::ToolSourceMismatch, Outcome::Unknown));
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn verify_linux(&self) -> Result<()> {
        self.validate()?;
        use super::diagnostic::{Category, Outcome, failure};
        if nix::unistd::geteuid().as_raw() != self.uid {
            return Err(failure(Category::ToolUidMismatch, Outcome::NotStarted));
        }
        if fs::read_link("/proc/self/exe")? != Path::new(&self.linux_program) {
            return Err(failure(Category::ToolImageMismatch, Outcome::NotStarted));
        }
        let root = Path::new(&self.linux_source_root);
        manifest::git_source(root, &self.source_commit)
            .map_err(|_| failure(Category::ToolSourceMismatch, Outcome::NotStarted))?;
        manifest::verify_compiled(root, &self.sources)
            .map_err(|_| failure(Category::ToolSourceMismatch, Outcome::NotStarted))?;
        if digest::file(Path::new(&self.linux_program))?.sha256 != self.sha256 {
            return Err(failure(Category::ToolImageMismatch, Outcome::NotStarted));
        }
        Ok(())
    }

    pub(super) fn arguments(
        &self,
        request: &Value,
        output: &Path,
        action: &str,
    ) -> Result<Vec<String>> {
        // 仅接收 evidence::command 经 from_request 校验后的工具绑定。
        let path = output.join("launch-request.json");
        Ok(vec![
            doc::text(&request["wsl"]["path"], "WSL 工具")?.into(),
            "--distribution".into(),
            self.distribution.clone(),
            "--user".into(),
            self.user.clone(),
            "--exec".into(),
            self.linux_program.clone(),
            "__linux-cache".into(),
            action.into(),
            "--request".into(),
            super::model::linux_path(path.to_str().ok_or("缓存路径不是UTF-8")?)?,
            "--sha256".into(),
            digest::file(&path)?.sha256,
        ])
    }
}

pub(super) fn from_request(request: &Value) -> Result<NativeTool> {
    let tool: NativeTool = serde_json::from_value(request["native_tool"].clone())?;
    tool.validate()?;
    if request["distribution"] != tool.distribution {
        return Err("Redis 与原生工具发行版不同".into());
    }
    Ok(tool)
}

pub(super) fn value(backend: &Path) -> Result<Value> {
    Ok(json!(NativeTool::capture(backend).map_err(|_| {
        super::diagnostic::failure(
            super::diagnostic::Category::ToolConfigurationRejected,
            super::diagnostic::Outcome::NotStarted,
        )
    })?))
}

pub(super) fn verify_remote(backend: &Path, wsl: &Value, tool: &NativeTool) -> Result<()> {
    tool.finish(backend)?;
    let program = super::model::plain(Path::new(doc::text(&wsl["path"], "WSL 工具")?))?;
    if digest::file(&program)?.sha256 != doc::text(&wsl["sha256"], "WSL 摘要")? {
        return Err("WSL 工具字节漂移".into());
    }
    let input = serde_json::to_vec(&json!({"format_version":2,"native_tool":tool}))?;
    if input.len() > 1024 * 1024 {
        return Err("缓存工具完整来源超过1MiB；不能截断清单".into());
    }
    let output = crate::process::command_output_with_input_timeout(
        backend,
        program.to_str().ok_or("WSL路径不是UTF-8")?,
        &[
            "--distribution",
            &tool.distribution,
            "--user",
            &tool.user,
            "--exec",
            &tool.linux_program,
            "__linux-cache",
            "describe",
        ],
        &[],
        Some(&input),
        std::time::Duration::from_secs(20),
    )?;
    if !output.status.success() {
        if let Ok(diagnostic) = super::diagnostic::Diagnostic::decode_output(&output.stdout) {
            return Err(Box::new(super::process::RemoteFailure {
                code: output.status.code(),
                diagnostic,
            }));
        }
        return Err(Box::new(super::process::ControlFailure(
            output.status.code(),
        )));
    }
    if output.stdout.len() > 16384 {
        return Err("缓存 Linux 工具预检失败".into());
    }
    let value = doc::parse_fragment(std::str::from_utf8(&output.stdout)?, "缓存工具实际来源")?;
    if value
        != json!({"format_version":2,"uid":tool.uid,"program":tool.linux_program,
        "sha256":tool.sha256,"source_commit":tool.source_commit,"source_sha256":tool.source_sha256})
    {
        return Err("缓存 Linux 工具 UID、ELF 或完整来源不同".into());
    }
    tool.finish(backend)?;
    if digest::file(&program)?.sha256 != doc::text(&wsl["sha256"], "WSL 摘要")? {
        return Err("WSL 工具预检后漂移".into());
    }
    Ok(())
}

pub(crate) fn review_value(value: &Value) -> Result<()> {
    let tool: NativeTool = serde_json::from_value(value.clone())?;
    tool.validate()
}

pub(crate) fn successor(original: &Value, tools: &Value) -> Result<Value> {
    let before = from_request(original)?;
    let after: NativeTool = serde_json::from_value(tools["native_tool"].clone())?;
    after.validate()?;
    let stable = |tool: &NativeTool| {
        json!([
            tool.distribution,
            tool.user,
            tool.uid,
            tool.program,
            tool.linux_program,
            tool.source_root,
            tool.linux_source_root
        ])
    };
    if stable(&before) != stable(&after)
        || tools["redis_server"]["distribution"] != original["distribution"]
        || tools["redis_server"]["resolved_path"] != original["executable"]
        || tools["redis_server"]["sha256"] != original["sha256"]
        || tools["wsl"]["path"].as_str().map(str::to_ascii_lowercase)
            != original["wsl"]["path"]
                .as_str()
                .map(str::to_ascii_lowercase)
    {
        return Err("缓存工具后继不得改变原资源、Linux入口、UID或双端路径".into());
    }
    let mut result = original.clone();
    result["wsl"] = tools["wsl"].clone();
    result["native_tool"] = serde_json::to_value(after)?;
    super::model::shape(&result)?;
    Ok(result)
}

pub(crate) fn tools_hash(process: &Value) -> Result<String> {
    if process["format_version"] == 2 {
        digest::plan_hash(&json!({"wsl":process["wsl"],"native_tool":process["native_tool"]}))
    } else {
        digest::plan_hash(&json!({"wsl":process["wsl"],"python":process["python"]}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_path_preserves_order_ignores_relative_and_rejects_first_unsafe_candidate() {
        let backend = crate::workspace::root_dir();
        std::fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let directory = tempfile::tempdir_in(backend.join(".local-tests")).unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let name = if cfg!(windows) { "wsl.exe" } else { "wsl" };
        let candidate = first.join(name);
        std::fs::write(&candidate, b"fixture").unwrap();
        std::fs::write(second.join(name), b"fixture").unwrap();
        let path =
            std::env::join_paths([Path::new(""), Path::new("relative"), &first, &second]).unwrap();
        assert_eq!(wsl_from_path(&path).unwrap(), candidate);
        std::fs::remove_file(&candidate).unwrap();
        assert_eq!(wsl_from_path(&path).unwrap(), second.join(name));
        std::fs::create_dir(&candidate).unwrap();
        let error = wsl_from_path(&path).unwrap_err();
        assert!(error.to_string().contains("PATH.wsl"));
        assert!(!error.to_string().contains("relative"));
        assert!(wsl_from_path(std::ffi::OsStr::new("relative")).is_err());
    }
}
