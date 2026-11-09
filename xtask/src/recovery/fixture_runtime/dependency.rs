use super::model::*;
use crate::{
    Result,
    process::{PreservedFailure, resolved_executable},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub(super) struct FrontendCommand<'a> {
    pub binding: &'a Value,
    pub arguments: &'a Value,
    pub environment: &'a Value,
    pub log: &'a Path,
    pub directory: &'a Path,
    pub timeout: u64,
    pub secrets: &'a [String],
}
pub(super) trait Dependencies {
    fn call(&mut self, operation: &str, args: Value) -> Result<Value>;
    fn frontend(&mut self, command: FrontendCommand<'_>) -> Result<Value>;
}
pub(super) struct Native {
    pub root: PathBuf,
}
impl Native {
    pub(super) fn new(root: &Path) -> Result<Self> {
        directory(root)?;
        Ok(Self {
            root: root.to_owned(),
        })
    }

    fn program(&self, name: &str) -> Result<PathBuf> {
        let requested = PathBuf::from(resolved_executable(&self.root, name)?);
        let names = if cfg!(windows) && requested.extension().is_none() {
            std::env::var("PATHEXT")
                .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
                .split(';')
                .map(|extension| requested.with_extension(extension.trim_start_matches('.')))
                .collect::<Vec<_>>()
        } else {
            vec![requested.clone()]
        };
        let directories = if requested.is_absolute() {
            vec![PathBuf::new()]
        } else {
            std::env::split_paths(&std::env::var_os("PATH").ok_or("浏览器工具缺少 PATH")?)
                .map(|path| self.root.join(path))
                .collect()
        };
        for directory in directories {
            for name in &names {
                let candidate = directory.join(name);
                let Ok(metadata) = candidate.metadata() else {
                    continue;
                };
                if !metadata.is_file() {
                    continue;
                }
                #[cfg(unix)]
                if nix::unistd::access(candidate.as_path(), nix::unistd::AccessFlags::X_OK).is_err()
                {
                    continue;
                }
                return Ok(candidate.canonicalize()?);
            }
        }
        Err(format!("浏览器依赖缺少可执行文件 {name}；请核对工具文件和操作系统 PATH").into())
    }

    fn tools(&self, frontend: &Path) -> Result<Value> {
        let corepack = self.program("corepack")?;
        let node = if cfg!(windows) {
            Some(self.program("node")?)
        } else {
            None
        };
        let home = std::env::var_os("COREPACK_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| frontend.join(".local-tests/corepack"));
        let home = self.root.join(home).canonicalize()?;
        Ok(json!({"corepack":corepack,"node":node,"corepack_home":home}))
    }
}
pub(super) fn clean_environment(command: &mut Command) {
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        if key.to_str().is_some_and(|s| {
            [
                "PATH",
                "PATHEXT",
                "SYSTEMROOT",
                "WINDIR",
                "COMSPEC",
                "HOME",
                "USERPROFILE",
                "HOMEDRIVE",
                "HOMEPATH",
                "TEMP",
                "TMP",
                "COREPACK_HOME",
            ]
            .contains(&s.to_ascii_uppercase().as_str())
        }) {
            command.env(key, value);
        }
    }
}
impl Dependencies for Native {
    fn call(&mut self, operation: &str, args: Value) -> Result<Value> {
        if operation == "worker-origin" {
            crate::recovery::clone::native::json::exact(&args, &[], "Worker 来源只读请求")?;
            return super::worker_origin::capture();
        }
        if operation == "bootstrap" {
            let mut boot =
                super::runtime::bootstrap(&path(&args["backend"])?, &path(&args["environment"])?)?;
            // 当前前端仍消费该字段；这里只解析受配置约束的程序路径，不启动解释器。
            boot["python"] = json!(self.program("python")?);
            return Ok(boot);
        }
        if matches!(operation, "settings" | "limits") {
            let bootstrap = path(&args["bootstrap"])?;
            let execution = path(&args["execution"])?;
            let boot = super::runtime::bootstrap(&self.root, &bootstrap)?;
            check(
                boot["execution"] == args["execution"],
                "浏览器配置执行根与冻结环境不符",
            )?;
            let config = path(&boot["config_dir"])?;
            let table = if operation == "settings" {
                crate::rate_limit::load_app_table(&execution, Some(&config), "test")
            } else {
                crate::rate_limit::settings(&execution, Some(&config), text(&args["app_env"])?)
            }
            .map_err(|error| {
                format!(
                    "浏览器配置读取失败（{}）：{}；请核对 {} 的环境文件与限流字段",
                    error.code(),
                    error.message(),
                    config.display()
                )
            })?;
            return Ok(serde_json::to_value(table)?);
        }
        if operation == "tools" {
            return self.tools(&path(&args["frontend"])?);
        }
        if operation == "frontend-build" {
            let root = path(&args["frontend"])?;
            let proof = crate::recovery::frontend_build::validate(&root, None, None)?;
            proof.verify()?;
            return Ok(json!({"path":root.join("dist/.vite/restore-build.json")}));
        }
        if operation == "verify" {
            return super::runtime::verify(&args);
        }
        if matches!(operation, "control" | "observe" | "worker") {
            return super::runtime::control(&args, operation == "observe", operation == "worker");
        }
        if matches!(operation, "completion" | "terminate") {
            crate::recovery::clone::native::json::exact(
                &args,
                &["directory", "scope_id"],
                "前端进程完成参数",
            )?;
            return crate::recovery::full_stack::worker::frontend_evidence(
                &path(&args["directory"])?,
                text(&args["scope_id"])?,
                operation == "terminate",
            );
        }
        Err("未知浏览器依赖操作".into())
    }
    fn frontend(&mut self, command: FrontendCommand<'_>) -> Result<Value> {
        super::process::run(self, command)
    }
}
pub(super) fn failure(message: &str, code: Option<i32>) -> Box<dyn std::error::Error> {
    Box::new(PreservedFailure::new(message.into(), code))
}
