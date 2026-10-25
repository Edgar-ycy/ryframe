//! 固定 WSL 文件观察命令；所有解析与判断在 Rust 中完成，不启动解释器。
use super::super::{super::json as document, model};
use crate::Result;
use serde_json::Value;
use std::{path::Path, time::Duration};

pub(super) struct Observer<'a> {
    backend: &'a Path,
    request: &'a Value,
}

impl<'a> Observer<'a> {
    pub(super) fn new(backend: &'a Path, request: &'a Value) -> Result<Self> {
        model::validate(backend, request, true)?;
        let result = Self { backend, request };
        let interpreter = document::text(&request["python"]["path"], "冻结 Linux 解释器路径")?;
        if result
            .read("/usr/bin/readlink", &["-f", interpreter])?
            .trim()
            != document::text(&request["python"]["executable"], "冻结 Linux 解释器映像")?
        {
            return Err("WSL 运行工具实际路径与冻结请求不同".into());
        }
        result.sha(
            document::text(&request["python"]["executable"], "冻结解释器")?,
            document::text(&request["python"]["sha256"], "冻结解释器摘要")?,
        )?;
        Ok(result)
    }

    pub(super) fn read(&self, program: &str, arguments: &[&str]) -> Result<String> {
        let output = self.command(program, arguments)?;
        if !output.status.success() {
            return Err(
                "WSL 固定只读命令未成功；检查发行版、读取权限与冻结工具，不能推断资源已消失".into(),
            );
        }
        String::from_utf8(output.stdout).map_err(|_| "WSL 观察不是 UTF-8".into())
    }

    fn command(&self, program: &str, arguments: &[&str]) -> Result<std::process::Output> {
        if !matches!(
            program,
            "/usr/bin/cat"
                | "/usr/bin/readlink"
                | "/usr/bin/sha256sum"
                | "/usr/bin/test"
                | "/usr/bin/stat"
                | "/usr/bin/redis-server"
        ) {
            return Err("只读缓存观察拒绝未知系统命令".into());
        }
        if program == "/usr/bin/redis-server" && arguments != ["--version"] {
            return Err("只读工具观察仅允许 Redis 固定版本命令".into());
        }
        let wsl = model::plain(Path::new(document::text(
            &self.request["wsl"]["path"],
            "WSL 工具",
        )?))?;
        let frozen = &self.request["wsl"]["sha256"];
        if super::super::super::digest::file(&wsl)?.sha256 != document::text(frozen, "WSL 摘要")?
        {
            return Err("WSL 工具摘要已变化".into());
        }
        let distribution = document::text(&self.request["distribution"], "WSL 发行版")?;
        let mut args = vec!["--distribution", distribution, "--exec", program];
        args.extend_from_slice(arguments);
        let output = crate::process::command_output_with_input_timeout(
            self.backend,
            wsl.to_str().ok_or("WSL 路径不是 UTF-8")?,
            &args,
            &[],
            None,
            Duration::from_secs(20),
        )
        .map_err(|_| "WSL 固定只读命令未完成；保留现场，禁止自动重放")?;
        if output.stdout.len() > 4 * 1024 * 1024
            || super::super::super::digest::file(&wsl)?.sha256
                != document::text(frozen, "WSL 摘要")?
        {
            return Err("WSL 观察超限或工具在观察期间变化".into());
        }
        Ok(output)
    }

    fn sha(&self, file: &str, expected: &str) -> Result<()> {
        let raw = self.read("/usr/bin/sha256sum", &[file])?;
        if raw.split_whitespace().next() != Some(expected) {
            return Err("WSL 冻结工具实际摘要不同".into());
        }
        Ok(())
    }

    pub(super) fn finish(&self) -> Result<()> {
        model::validate(self.backend, self.request, true)?;
        self.sha(
            document::text(&self.request["python"]["executable"], "冻结解释器")?,
            document::text(&self.request["python"]["sha256"], "冻结解释器摘要")?,
        )
    }
}
