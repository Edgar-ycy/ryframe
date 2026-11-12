//! 正式 consumer 文件协议与后端 Python 检查的原生工具绑定；仅真实执行阶段进入。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{
    Result,
    process::{PreservedFailure, failure_exit_code},
};

#[path = "consumer_context/files.rs"]
mod files;
#[path = "consumer_context/identity.rs"]
mod identity;
#[path = "consumer_context/origin.rs"]
mod origin;
#[path = "consumer_context/python.rs"]
pub(crate) mod python;

pub(crate) use files::HeldFile;
pub(crate) use files::selected_frontend_directory;
use files::{Binding, Source};
pub(crate) use origin::{OriginBinding, existing as existing_origin};

const CONFIGURATION: [&str; 2] = ["config/tools.toml", ".local-tests/tools.toml"];

struct Configuration {
    rows: Vec<Source>,
    existing: Vec<HeldFile>,
    absent: Vec<PathBuf>,
}

impl Configuration {
    fn capture(root: &Path) -> Result<Self> {
        let mut configuration = Self {
            rows: Vec::new(),
            existing: Vec::new(),
            absent: Vec::new(),
        };
        for relative in CONFIGURATION {
            let path = root.join(relative);
            if files::exists(&path)? {
                let file = HeldFile::open(&path)?;
                configuration.rows.push(Source {
                    path: relative.into(),
                    sha256: file.sha256.clone(),
                });
                configuration.existing.push(file);
            } else {
                configuration.absent.push(path);
            }
        }
        Ok(configuration)
    }

    fn verify(&self) -> Result<()> {
        for file in &self.existing {
            file.verify()?;
        }
        for path in &self.absent {
            if files::exists(path)? {
                return Err("工具配置存在性漂移：保留上下文并使用一致配置重新执行。".into());
            }
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct Document<'a> {
    format_version: u32,
    arguments: &'a [String],
    backend_root: &'a Path,
    frontend_root: &'a Path,
    origin_tool_binding: Binding,
    tool_configuration: &'a [Source],
}

pub(crate) struct ConsumerContext {
    path: PathBuf,
    configuration: Configuration,
    origin: origin::OriginBinding,
    document: HeldFile,
}

impl ConsumerContext {
    pub(crate) fn path_text(&self) -> Result<&str> {
        self.path
            .to_str()
            .ok_or_else(|| "consumer context 路径必须为 UTF-8。".into())
    }

    pub(crate) fn verify(&self) -> Result<()> {
        let result = (|| {
            self.configuration.verify()?;
            self.origin.verify()?;
            self.document.verify()
        })();
        if result.is_err() {
            record_failure(self.path.parent().ok_or(files::FILE_ERROR)?);
        }
        result
    }

    pub(crate) fn finish(self, result: Result<()>) -> Result<()> {
        let verification = self.verify();
        if result.is_err() || verification.is_err() {
            record_failure(self.path.parent().ok_or(files::FILE_ERROR)?);
        }
        finish_verified_result(result, verification)
    }
}

fn finish_verified_result(result: Result<()>, verification: Result<()>) -> Result<()> {
    match (result, verification) {
        (Err(error), Err(verification)) => {
            let mut preserved =
                PreservedFailure::new(error.to_string(), failure_exit_code(error.as_ref()));
            preserved.append_context(&verification.to_string());
            Err(preserved.into())
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

/// 独立发布固定 origin 绑定，不读取前端配置或路径。
pub(crate) fn prepare_origin(root: &Path) -> Result<OriginBinding> {
    let (root, evidence) = origin_run(root)?;
    let result = origin::prepare(&root, &evidence);
    if result.is_err() {
        record_failure(&evidence);
    }
    result
}

fn origin_run(root: &Path) -> Result<(PathBuf, PathBuf)> {
    let root = files::absolute_directory(root)?;
    let directory = root.join(".local-tests/origin-runs");
    files::directory(&directory)?;
    let evidence = directory.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&evidence).map_err(|_| files::FILE_ERROR)?;
    files::no_links(&evidence)?;
    Ok((root, evidence))
}

pub(crate) fn prepare(
    root: &Path,
    frontend: &Path,
    arguments: Vec<String>,
) -> Result<ConsumerContext> {
    let root = files::absolute_directory(root)?;
    let frontend = files::absolute_directory(frontend)?;
    let directory = root.join(".local-tests/consumer-contexts");
    files::directory(&directory)?;
    let evidence = directory.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&evidence).map_err(|_| files::FILE_ERROR)?;
    let result = prepare_in(&root, &frontend, arguments, &evidence);
    if result.is_err() {
        record_failure(&evidence);
    }
    result
}

fn prepare_in(
    root: &Path,
    frontend: &Path,
    arguments: Vec<String>,
    evidence: &Path,
) -> Result<ConsumerContext> {
    validate_arguments(&arguments)?;
    let configuration = Configuration::capture(root)?;
    let origin = origin::prepare(root, evidence)?;
    let bytes = encode(
        root,
        frontend,
        &arguments,
        &configuration.rows,
        origin.binding.binding()?,
    )?;
    configuration.verify()?;
    origin.verify()?;
    let path = evidence.join("context.json");
    files::write_new(&path, &bytes)?;
    let document = HeldFile::open(&path)?;
    let context = ConsumerContext {
        path,
        configuration,
        origin,
        document,
    };
    context.verify()?;
    Ok(context)
}

fn validate_arguments(arguments: &[String]) -> Result<()> {
    let names = [
        "--mode",
        "--openapi",
        "--backend-commit",
        "--backend-repository",
        "--require-pin",
    ];
    if arguments.len() != names.len() * 2
        || !arguments
            .as_chunks::<2>()
            .0
            .iter()
            .zip(names)
            .all(|(pair, name)| pair[0] == name && !pair[1].is_empty())
    {
        return Err("consumer 参数不符合固定五对 CLI 协议：请由当前契约计划生成完整参数。".into());
    }
    Ok(())
}

fn encode(
    root: &Path,
    frontend: &Path,
    arguments: &[String],
    configuration: &[Source],
    binding: Binding,
) -> Result<Vec<u8>> {
    validate_arguments(arguments)?;
    if !root.is_absolute() || !frontend.is_absolute() || !binding.path.is_absolute() {
        return Err("consumer context 必须绑定绝对工作区和 origin 工具路径。".into());
    }
    if binding.path != root.join(".local-tests/frontend-origin-tool.json")
        || binding.bytes == 0
        || binding.bytes > 9_007_199_254_740_991
        || !files::digest_valid(&binding.sha256)
    {
        return Err("consumer origin 绑定必须使用固定文件、正安全整数长度和小写 SHA-256。".into());
    }
    let mut previous = None;
    for source in configuration {
        let index = CONFIGURATION.iter().position(|path| *path == source.path);
        let Some(index) = index else {
            return Err("consumer 工具配置来源不属于固定公私文件集合。".into());
        };
        if previous.is_some_and(|previous| previous >= index)
            || !files::digest_valid(&source.sha256)
        {
            return Err("consumer 工具配置来源顺序或摘要无效。".into());
        }
        previous = Some(index);
    }
    Ok(serde_json::to_vec(&Document {
        format_version: 1,
        arguments,
        backend_root: root,
        frontend_root: frontend,
        origin_tool_binding: binding,
        tool_configuration: configuration,
    })?)
}

fn record_failure(evidence: &Path) {
    let _ = files::write_new(
        &evidence.join("failure.json"),
        b"{\"format_version\":1,\"valid\":false,\"error\":\"consumer_context_failed\"}\n",
    );
}

#[cfg(test)]
#[path = "../../tests/internal/consumer_context/tests.rs"]
mod tests;
