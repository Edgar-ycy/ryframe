use std::path::PathBuf;

use ryframe_kernel::{AppError, AppResult};

/// 提取公共配置目录参数，保留其余参数供具体命令继续解析。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationArgs {
    pub directory: PathBuf,
    pub arguments: Vec<String>,
}

impl ConfigurationArgs {
    /// 纯参数解析；不读取环境或文件，也不决定具体命令是否需要加载配置。
    pub fn parse(arguments: Vec<String>) -> AppResult<Self> {
        let mut directory = None;
        let mut remaining = Vec::new();
        let mut input = arguments.into_iter().peekable();
        while let Some(argument) = input.next() {
            if argument == "--" {
                remaining.push(argument);
                remaining.extend(input);
                break;
            }
            if argument.starts_with("--config-dir=") {
                return Err(AppError::Config(
                    "不支持 --config-dir=...；请使用 --config-dir <目录> 格式".into(),
                ));
            }
            if argument != "--config-dir" {
                remaining.push(argument);
                continue;
            }
            if directory.is_some() || input.peek().is_some_and(|value| value == "--config-dir") {
                return Err(AppError::Config("--config-dir 最多只能指定一次".into()));
            }
            let value = input
                .next()
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| {
                    AppError::Config(
                        "--config-dir 缺少目录；请使用 --config-dir <目录> 格式".into(),
                    )
                })?;
            if value.trim().is_empty()
                || value
                    .chars()
                    .any(|value| matches!(value, '\0' | '\r' | '\n'))
            {
                return Err(AppError::Config(
                    "--config-dir 目录必须非空且不得包含 NUL、CR 或 LF".into(),
                ));
            }
            directory = Some(PathBuf::from(value));
        }
        Ok(Self {
            directory: directory.unwrap_or_else(|| PathBuf::from("config")),
            arguments: remaining,
        })
    }
}
