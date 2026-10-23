use super::model::*;
use crate::{
    Result,
    process::{ChildGroup, PreservedFailure, child_command, resolved_executable},
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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
pub(super) struct Python {
    pub root: PathBuf,
    pub executable: PathBuf,
}
impl Python {
    pub(super) fn new(root: &Path) -> Result<Self> {
        Ok(Self {
            root: root.to_owned(),
            executable: PathBuf::from(resolved_executable(root, "python")?),
        })
    }
    pub(super) fn command(&self) -> Command {
        let mut command = child_command(&self.executable);
        command.args(["-B", "-X", "utf8"]).arg(
            self.root
                .join("tools/python/fixture_browser_dependencies.py"),
        );
        command.current_dir(&self.root);
        clean_environment(&mut command);
        command
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
impl Dependencies for Python {
    fn call(&mut self, operation: &str, args: Value) -> Result<Value> {
        if operation == "worker" {
            check(
                args["action"] == "start",
                "浏览器仅通过现有Worker协议恢复已登记运行时",
            )?;
            return super::worker_handoff::start(self, &args);
        }
        let mut input = tempfile::tempfile_in(self.root.join(".local-tests"))?;
        let mut native_tool = bound(&std::env::current_exe()?)?;
        native_tool["format_version"] = json!(1);
        native_tool["kind"] = json!("ryframe-native-rate-limit-tool");
        input.write_all(&serde_json::to_vec(
            &json!({"operation":operation,"arguments":args,"native_tool":native_tool}),
        )?)?;
        use std::io::{Seek, SeekFrom};
        input.seek(SeekFrom::Start(0))?;
        let mut command = self.command();
        command
            .stdin(input)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = ChildGroup::new()?.spawn(command)?;
        // 依赖适配器输入只含当前调用值；输出不进入日志，凭据仅驻留内存。
        let stdout = child.take_stdout_reader()?.ok_or("浏览器依赖缺少输出")?;
        let read = std::thread::spawn(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            stdout
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
        let status = child.wait()?;
        let bytes = read.join().map_err(|_| "浏览器依赖读取失败")??;
        check(
            status.success() && bytes.len() <= 16 * 1024 * 1024,
            "浏览器依赖调用失败；核对原运行收据",
        )?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "浏览器依赖返回无效JSON")?;
        check(value["ok"] == true, "浏览器依赖拒绝当前输入")?;
        Ok(value["value"].clone())
    }
    fn frontend(&mut self, command: FrontendCommand<'_>) -> Result<Value> {
        super::process::run(self, command)
    }
}
pub(super) fn failure(message: &str, code: Option<i32>) -> Box<dyn std::error::Error> {
    Box::new(PreservedFailure::new(message.into(), code))
}
