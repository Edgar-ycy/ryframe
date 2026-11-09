//! 受限上下文与 OS 原语边界；私有响应只进入内存，不转发日志。
use super::{
    Runtime,
    model::{self, check},
};
use crate::{Result, recovery::clone::native::json as json_io};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

pub(super) const LIMIT: usize = 4 * 1024 * 1024;
pub(super) struct LifecycleRuntime {
    backend: PathBuf,
    tool: Value,
    started: Instant,
}
impl LifecycleRuntime {
    pub(super) fn new(backend: &Path) -> Result<Self> {
        let executable = std::env::current_exe()?;
        let tool = crate::check::python_suite::binding(&executable)?
            .ok_or("服务生命周期需要明确原生工具绑定")?;
        Ok(Self {
            backend: backend.to_path_buf(),
            tool: serde_json::from_str(&tool)?,
            started: Instant::now(),
        })
    }
    fn invoke(&self, operation: &str, arguments: Value) -> Result<Value> {
        super::primitive::validate(&self.backend, operation, &arguments)?;
        self.verify_tool()?;
        let payload = serde_json::to_vec(
            &json!({"format_version":1,"kind":"ryframe-fixture-service-primitive",
            "backend":self.backend,"native_tool":self.tool,"operation":operation,"arguments":arguments}),
        )?;
        check(payload.len() <= LIMIT, "服务原语请求超过 4 MiB")?;
        let local = self.backend.join(".local-tests");
        let mut input = tempfile::tempfile_in(&local)?;
        input.write_all(&payload)?;
        input.seek(SeekFrom::Start(0))?;
        let mut output = tempfile::tempfile_in(&local)?;
        let mut command = crate::process::child_command(model::text(&self.tool, "path")?);
        command
            .arg(super::cache_primitive::ENTRY)
            .current_dir(&self.backend)
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(output.try_clone()?))
            .stderr(Stdio::null());
        restricted_environment(&mut command);
        // 服务监督器自行持有登记的 Job/session；不能因短期原语进程结束而终止服务。
        let mut child = command
            .spawn()
            .map_err(|_| model::error("无法启动服务原语；尚未取得执行结果"))?;
        let status = match wait_primitive(&mut child, &output) {
            Ok(status) => status,
            Err(error) => {
                // 只回收本次确切句柄；独立监督器及可能已写入的服务保持未知现场。
                let killed = child.kill();
                let waited = child.wait();
                if killed.is_err() || waited.is_err() {
                    return Err(model::error("原语进程回收未确认；保留完整现场"));
                }
                return Err(error);
            }
        };
        let response = read_response(&mut output)?;
        self.verify_tool()?;
        super::cache_primitive::decode(status.success(), &response)
    }

    fn verify_tool(&self) -> Result<()> {
        check(
            self.tool == crate::check::python_suite::native_tool_binding()?,
            "原生cache原语当前工具字节或身份变化；保留现场",
        )
    }
}

fn wait_primitive(
    child: &mut std::process::Child,
    output: &File,
) -> Result<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        if output.metadata()?.len() > LIMIT as u64 || started.elapsed() > Duration::from_secs(180) {
            return Err(Box::new(model::LifecycleError {
                category: "PrimitiveUnknown",
                message: "服务原语超时或输出超限，写入结果未知；禁止自动重放",
            }));
        }
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
impl Runtime for LifecycleRuntime {
    fn call(&mut self, operation: &str, arguments: Value) -> Result<Value> {
        if operation == "cache-closed-port" {
            json_io::exact(&arguments, &["request", "runtime"], "只读独占端口")?;
            crate::recovery::clone::native::cache::host_control::closed_port(
                &self.backend,
                &arguments["request"],
                &arguments["runtime"],
            )?;
            return Ok(json!(true));
        }
        if matches!(operation, "cache-start" | "cache-stop" | "storage-start") {
            return self.invoke(operation, arguments);
        }
        if matches!(operation, "tree-stop" | "recover-controller") {
            super::primitive::validate(&self.backend, operation, &arguments)?;
            return match operation {
                "tree-stop" => super::tree_control::stop(
                    &self.backend,
                    &arguments["tree"],
                    arguments["crash"].as_bool().ok_or("crash 必须为布尔值")?,
                ),
                _ => super::controller_recovery::run(
                    &self.backend,
                    Path::new(model::text(&arguments, "run")?),
                    &arguments["owner"],
                ),
            };
        }
        if super::readonly::handles(operation) {
            return super::readonly::NativeRuntime::new(&self.backend).call(operation, arguments);
        }
        if operation == "context" {
            let backend = self.backend.clone();
            super::context::load(
                self,
                &backend,
                Path::new(model::text(&arguments, "review")?),
                Path::new(model::text(&arguments, "environment")?),
            )
        } else if operation == "registered" {
            let backend = self.backend.clone();
            super::registration::load(
                self,
                &backend,
                &arguments["context"],
                arguments["settled"]
                    .as_bool()
                    .ok_or("settled 必须为布尔值")?,
            )
        } else if operation == "observe" {
            let backend = self.backend.clone();
            super::observation::observe(self, &backend, &arguments["services"])
        } else {
            Err(model::error("未知服务原语；不启动子进程"))
        }
    }
    fn now(&self) -> Duration {
        self.started.elapsed()
    }
    fn pause(&mut self, delay: Duration) {
        std::thread::sleep(delay);
    }
}

fn read_response(file: &mut File) -> Result<Value> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take((LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    check(bytes.len() <= LIMIT, "服务原语响应超过 4 MiB")?;
    let text = std::str::from_utf8(&bytes).map_err(|_| model::error("服务原语响应不是 UTF-8"))?;
    json_io::parse_fragment(text, "服务原语响应")
        .map_err(|_| model::error("服务原语响应无效；不回显私有输出"))
}

pub(super) fn decode(success: bool, value: &Value) -> Result<Value> {
    if success && value["ok"] == true {
        json_io::exact(value, &["ok", "result"], "服务原语成功响应")?;
        return Ok(value["result"].clone());
    }
    json_io::exact(value, &["ok", "error"], "服务原语失败响应")?;
    check(
        !success && value["ok"] == false,
        "服务原语协议与退出状态不一致",
    )?;
    let category = match value["error"].as_str() {
        Some("PermissionError") => "PermissionError",
        Some("TimeoutError") => "TimeoutError",
        Some("ValueError") => "ValueError",
        _ => "PrimitiveUnknown",
    };
    Err(Box::new(model::LifecycleError {
        category,
        message: "服务原语未完成；检查绑定的私有输入与既有证据，不自动重放",
    }))
}

fn restricted_environment(command: &mut std::process::Command) {
    let safe = std::env::vars_os()
        .filter(|(key, _)| {
            let key = key.to_string_lossy().to_ascii_uppercase();
            [
                "PATH",
                "PATHEXT",
                "SYSTEMROOT",
                "WINDIR",
                "COMSPEC",
                "TEMP",
                "TMP",
                "TMPDIR",
                "HOME",
                "USERPROFILE",
                "HOMEDRIVE",
                "HOMEPATH",
                "LANG",
            ]
            .contains(&key.as_str())
                || key.starts_with("CARGO_")
                || key.starts_with("RUSTUP_")
                || key.starts_with("LC_")
        })
        .collect::<Vec<_>>();
    command.env_clear().envs(safe);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_response_requires_exact_envelope_exit_status_and_no_secret_echo() {
        assert_eq!(
            decode(true, &json!({"ok":true,"result":{"value":1}})).unwrap(),
            json!({"value":1})
        );
        for (success, response) in [
            (false, json!({"ok":true,"result":"private-marker"})),
            (true, json!({"ok":false,"error":"private-marker"})),
            (
                true,
                json!({"ok":true,"result":null,"secret":"private-marker"}),
            ),
            (false, json!({"ok":false,"error":"private-marker"})),
        ] {
            let error = decode(success, &response).unwrap_err();
            assert!(!error.to_string().contains("private-marker"));
        }
        let error = decode(false, &json!({"ok":false,"error":"PermissionError"})).unwrap_err();
        assert_eq!(model::category(error.as_ref()), "PermissionError");
    }

    #[test]
    fn unexpected_primitive_fails_before_opening_a_child() {
        let mut runtime = LifecycleRuntime {
            backend: PathBuf::from("unused-backend"),
            tool: Value::Null,
            started: Instant::now(),
        };
        let error = runtime.call("unexpected", json!({})).unwrap_err();
        assert_eq!(
            model::reason(error.as_ref()),
            Some("未知服务原语；不启动子进程")
        );
    }

    #[test]
    fn private_response_is_bounded_and_does_not_echo_invalid_bytes() {
        let mut output =
            tempfile::tempfile_in(crate::workspace::root_dir().join(".local-tests")).unwrap();
        output.write_all(b"private-marker").unwrap();
        assert!(
            !read_response(&mut output)
                .unwrap_err()
                .to_string()
                .contains("private-marker")
        );
        output.set_len(LIMIT as u64 + 1).unwrap();
        assert!(read_response(&mut output).is_err());
    }
}
