use std::{collections::BTreeMap, fs, path::Path, time::Duration};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::{
    super::{
        digest,
        export::transport::{CommandOutput, CommandSpec, StdoutTarget, Transport},
        json::{self as json_io, text},
    },
    input::request_physical,
    storage::StorageInputs,
};
use crate::{Result, cli::CloneSide};

#[path = "stdout.rs"]
pub(crate) mod stdout;

/// 交接复用固定 identity/ownership 查询及原有空输出核验规则，不接受任意 SQL。
pub(crate) fn verify_query<T: Transport, F: Fn() -> Result<()>>(
    context: &StorageInputs<'_>,
    configuration: &Value,
    transport: &T,
    checkpoint: &F,
    database: &Value,
    check: &str,
) -> Result<String> {
    verify_query_for_side(
        context,
        configuration,
        CloneSide::Target,
        transport,
        checkpoint,
        database,
        check,
    )
}

/// 两侧交接只复用固定查询，按原配置根保留输出及一次空输出重读。
pub(crate) fn verify_query_for_side<T: Transport, F: Fn() -> Result<()>>(
    context: &StorageInputs<'_>,
    configuration: &Value,
    side: CloneSide,
    transport: &T,
    checkpoint: &F,
    database: &Value,
    check: &str,
) -> Result<String> {
    let port = Port {
        configuration: Some(configuration),
        side,
        context,
        transport,
        checkpoint,
        directory: context.observation_dir,
    };
    Evidence::default().mysql(&port, database, check)
}
#[cfg(test)]
#[path = "../../../../../../tests/internal/target_stdout_native.rs"]
mod stdout_tests;

#[derive(Debug)]
struct CommandFailure {
    cause: Box<dyn std::error::Error>,
    kind: &'static str,
    returncode: Option<i32>,
    stderr: Option<Vec<u8>>,
}

impl CommandFailure {
    fn boxed(
        cause: Box<dyn std::error::Error>,
        kind: &'static str,
        result: &Result<CommandOutput>,
    ) -> Box<dyn std::error::Error> {
        Box::new(Self {
            cause,
            kind,
            returncode: result.as_ref().ok().and_then(|output| output.status.code()),
            stderr: result.as_ref().ok().map(|output| output.stderr.clone()),
        })
    }

    fn transport(cause: Box<dyn std::error::Error>) -> Box<dyn std::error::Error> {
        Box::new(Self {
            cause,
            kind: "TransportError",
            returncode: None,
            stderr: None,
        })
    }
}

impl std::fmt::Display for CommandFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "数据库命令未完成：{}，原始输出与分层诊断已保留",
            self.kind
        )
    }
}
impl std::error::Error for CommandFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

#[derive(Default)]
pub(super) struct Evidence {
    files: BTreeMap<String, Value>,
}

pub(super) struct Port<'a, T, F> {
    pub(super) configuration: Option<&'a Value>,
    pub(super) side: CloneSide,
    pub(super) context: &'a StorageInputs<'a>,
    pub(super) transport: &'a T,
    pub(super) checkpoint: &'a F,
    pub(super) directory: &'a Path,
}

#[derive(Debug)]
pub(super) struct VerificationError {
    pub(super) details: Value,
}

impl std::fmt::Display for VerificationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("数据库固定只读核验失败，安全诊断已绑定")
    }
}

impl std::error::Error for VerificationError {}

fn verification_error(
    check: &str,
    database: &Value,
    response: &[u8],
    measurement: &str,
    returncode: Option<i32>,
) -> Box<dyn std::error::Error> {
    Box::new(VerificationError {
        details: json!({
            "code": format!("database_{check}_{}", if returncode.is_some() { "command_failed" } else { "mismatch" }),
            "check": check, "target_key": database["key"], "expected_rows": if check == "ownership" && database["kind"] == "combined" { 2 } else { 1 },
            "actual_rows": String::from_utf8_lossy(response).lines().count(), "response_bytes": response.len(),
            "response_measurement": measurement, "returncode": returncode,
        }),
    })
}

pub(super) fn mismatch(
    check: &str,
    database: &Value,
    response: &str,
) -> Box<dyn std::error::Error> {
    verification_error(
        check,
        database,
        response.as_bytes(),
        "mysql_utf8_after_strip",
        None,
    )
}

pub(super) fn descriptor(path: &Path) -> Result<Value> {
    let actual = digest::file(path)?;
    Ok(json!({"bytes": actual.bytes, "sha256": actual.sha256}))
}

fn bytes_descriptor(bytes: &[u8]) -> Value {
    json!({"bytes": bytes.len(), "sha256": hex::encode(Sha256::digest(bytes))})
}

impl Evidence {
    pub(super) fn track(&mut self, directory: &Path, name: &str) -> Result<()> {
        if Path::new(name).file_name().and_then(|name| name.to_str()) != Some(name) {
            return Err("库存证据越出固定平面目录".into());
        }
        let value = descriptor(&directory.join(name))?;
        if self.files.insert(name.into(), value).is_some() {
            return Err("库存证据名称重复".into());
        }
        Ok(())
    }

    pub(super) fn write(&mut self, directory: &Path, name: &str, value: &Value) -> Result<()> {
        json_io::write_new(&directory.join(name), value)?;
        self.track(directory, name)
    }

    pub(super) fn files(&self, directory: &Path) -> Result<Vec<Value>> {
        if digest::linked_path(directory)?
            .into_iter()
            .any(|linked| linked)
        {
            return Err("库存证据目录不能经过链接".into());
        }
        let mut actual = BTreeMap::new();
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "库存证据文件名必须为 UTF-8")?;
            actual.insert(name, descriptor(&entry.path())?);
        }
        if actual != self.files {
            return Err("库存目录包含未声明文件、目录或已变化证据".into());
        }
        Ok(actual.into_iter().map(|(name, binding)| json!({"path": name, "type": "file", "bytes": binding["bytes"], "sha256": binding["sha256"]})).collect())
    }

    pub(super) fn inventory<T: Transport, F: Fn() -> Result<()>>(
        &mut self,
        port: &Port<'_, T, F>,
        output: &Path,
    ) -> Result<()> {
        if fs::symlink_metadata(output).is_ok() {
            return Err("库存 CLI 不能覆盖已有原始输出".into());
        }
        if output.parent() != Some(port.directory) {
            return Err("库存输出越出固定采集目录".into());
        }
        let key = output
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.split_once("-target-").map(|(_, key)| key))
            .ok_or("库存输出文件名无效")?;
        let args = super::super::resume_prefix::configured_command(
            port.context
                .maintenance_executable("tenant-data")?
                .to_str()
                .ok_or("库存工具路径不是UTF8")?,
            port.context.request,
            vec![
                "target-inventory".into(),
                "--target".into(),
                key.into(),
                "--output".into(),
                output.display().to_string(),
            ],
        )?
        .into_iter()
        .skip(1)
        .collect();
        let result = self.command(
            port,
            port.context.maintenance_executable("tenant-data")?,
            args,
            None,
        )?;
        if !result.status.success() {
            return Err("目标库存 CLI 执行失败，诊断已绑定".into());
        }
        Ok(())
    }

    pub(super) fn mysql<T: Transport, F: Fn() -> Result<()>>(
        &mut self,
        port: &Port<'_, T, F>,
        database: &Value,
        check: &str,
    ) -> Result<String> {
        let mut receipts = Vec::new();
        let first = self.mysql_attempt(port, database, check, &mut receipts)?;
        if !first.stdout.is_empty() || !first.stderr.is_empty() {
            return response(&first);
        }
        let second = self.mysql_attempt(port, database, check, &mut receipts);
        if receipts.len() != 2 {
            second?;
            return Err("MySQL 重试缺少完整两次输出收据".into());
        }
        self.write(port.directory, &format!("mysql-{check}-retry-{}.json", uuid::Uuid::new_v4().simple()), &json!({
            "format_version": 1, "kind": "mysql-verification-retry", "check": check,
            "target_key": database["key"], "reason": "first_returncode_zero_stdout_stderr_exactly_empty",
            "first": receipts[0], "second": receipts[1],
        }))?;
        let second = second?;
        if second.stdout.is_empty() {
            return Err(mismatch(check, database, ""));
        }
        response(&second)
    }

    fn mysql_attempt<T: Transport, F: Fn() -> Result<()>>(
        &mut self,
        port: &Port<'_, T, F>,
        database: &Value,
        check: &str,
        receipts: &mut Vec<Value>,
    ) -> Result<CommandOutput> {
        let args = mysql_arguments(port.context, database, check)?;
        let name = format!("mysql-{check}-{}", uuid::Uuid::new_v4().simple());
        let mut output =
            stdout::VerifiedStdout::create(&port.directory.join(format!("{name}.stdout")))?;
        let result = self.command(port, port.context.tool("mysql")?, args, Some(&mut output));
        let snapshot = output.snapshot();
        let receipt = mysql_receipt(database, check, &output, &result, &snapshot);
        let receipt_name = format!("{name}.json");
        self.write(port.directory, &receipt_name, &receipt)?;
        let bound = descriptor(&port.directory.join(&receipt_name))?;
        receipts.push(json!({"path": port.directory.join(receipt_name), "bytes": bound["bytes"], "sha256": bound["sha256"]}));
        let mut result = result?;
        result.stdout = snapshot?.raw;
        if !result.status.success() {
            return Err(verification_error(
                check,
                database,
                &result.stdout,
                "raw_stdout",
                result.status.code(),
            ));
        }
        Ok(result)
    }

    fn command<T: Transport, F: Fn() -> Result<()>>(
        &mut self,
        port: &Port<'_, T, F>,
        program: std::path::PathBuf,
        args: Vec<String>,
        stdout: Option<&mut stdout::VerifiedStdout>,
    ) -> Result<CommandOutput> {
        (port.checkpoint)()?;
        let mut environment = port.context.command_environment();
        environment.remove("MYSQL_PWD");
        let login = port.directory.join("unused-login.cnf");
        if fs::symlink_metadata(&login).is_ok() {
            return Err("库存不读取隐式 MySQL login-path 文件".into());
        }
        environment.insert("MYSQL_TEST_LOGIN_FILE".into(), login.display().to_string());
        let mut command = vec![program.display().to_string()];
        command.extend(args.clone());
        let secrets = secrets(port.context, port.configuration, port.side)?;
        let target = match &stdout {
            Some(output) => output.target()?,
            None => StdoutTarget::Capture,
        };
        let mut result = port.transport.run(CommandSpec {
            program,
            args,
            cwd: port.context.execution_backend()?,
            environment,
            stdin: None,
            stdout: target,
            work_dir: port.directory.to_path_buf(),
            label: "目标数据库只读核验".into(),
            timeout: Duration::from_secs(1800),
        });
        let capture_error =
            self.record_command(port.directory, &command, &secrets, &mut result, stdout)?;
        if let Err(error) = (port.checkpoint)() {
            return Err(CommandFailure::boxed(error, "CheckpointError", &result));
        }
        let result = result.map_err(CommandFailure::transport)?;
        if let Some(error) = capture_error {
            let kind = error.kind();
            return Err(CommandFailure::boxed(Box::new(error), kind, &Ok(result)));
        }
        Ok(result)
    }

    fn record_command(
        &mut self,
        directory: &Path,
        command: &[String],
        secrets: &[String],
        result: &mut Result<CommandOutput>,
        stdout: Option<&mut stdout::VerifiedStdout>,
    ) -> Result<Option<stdout::CaptureError>> {
        let mut receipt = json!({"command": command, "returncode": null, "error_type": null, "stdin": null, "stdout": null, "stderr": ""});
        match result {
            Ok(output) => {
                receipt["returncode"] = json!(output.status.code());
                receipt["stdout"] = json!(redact(&output.stdout, secrets));
                receipt["stderr"] = json!(redact(&output.stderr, secrets));
            }
            Err(_) => receipt["error_type"] = json!("TransportError"),
        }
        let mut failure = None;
        if let Some(stdout) = stdout {
            receipt["stdout_file"] = json!({"path": stdout.path()});
            match stdout.snapshot() {
                Ok(snapshot) => {
                    self.bind_stdout(directory, &snapshot.artifact)?;
                    receipt["stdout_file"] = snapshot.artifact;
                    receipt["stdout"] = json!(redact(&snapshot.raw, secrets));
                    if let Ok(output) = result {
                        output.stdout = snapshot.raw;
                    }
                }
                Err(error) => {
                    receipt["stdout"] = Value::Null;
                    receipt["stdout_capture_error"] = json!(error.kind());
                    failure = Some(error);
                }
            }
        }
        self.write(
            directory,
            &format!("command-{}.json", uuid::Uuid::new_v4().simple()),
            &receipt,
        )?;
        Ok(failure)
    }

    fn bind_stdout(&mut self, directory: &Path, artifact: &Value) -> Result<()> {
        let path = Path::new(text(&artifact["path"], "原始输出路径")?);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("原始输出文件名无效")?;
        if path.parent() != Some(directory) || self.files.contains_key(name) {
            return Err("原始输出越界或重复登记".into());
        }
        self.files.insert(
            name.into(),
            json!({"bytes": artifact["bytes"], "sha256": artifact["sha256"]}),
        );
        Ok(())
    }
}

fn mysql_arguments(
    context: &StorageInputs<'_>,
    database: &Value,
    check: &str,
) -> Result<Vec<String>> {
    let sql = verification_sql(check)?;
    request_physical::defaults_connection(context.backend, database)?;
    let mut args = mysql_client_arguments(database)?;
    args.extend(["--execute".to_owned(), sql.to_owned()]);
    Ok(args)
}

pub(crate) fn mysql_client_arguments(database: &Value) -> Result<Vec<String>> {
    Ok(vec![
        format!(
            "--defaults-file={}",
            text(&database["defaults_file"], "数据库凭据路径")?
        ),
        format!("--database={}", text(&database["database"], "物理库名")?),
        "--batch".into(),
        "--raw".into(),
        "--skip-column-names".into(),
        "--binary-mode".into(),
        "--default-character-set=utf8mb4".into(),
    ])
}

pub(crate) fn verification_sql(check: &str) -> Result<&'static str> {
    Ok(match check {
        "identity" => "SELECT @@server_uuid, DATABASE();",
        "ownership" => {
            "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;"
        }
        _ => return Err("数据库核验只允许固定 identity 或 ownership 查询".into()),
    })
}

pub(crate) fn mysql_receipt(
    database: &Value,
    check: &str,
    output: &stdout::VerifiedStdout,
    result: &Result<CommandOutput>,
    snapshot: &std::result::Result<stdout::Snapshot, stdout::CaptureError>,
) -> Value {
    let (code, stderr, error_type) = match result {
        Ok(output) => (
            output.status.code(),
            Some(output.stderr.as_slice()),
            if output.status.success() {
                None
            } else {
                Some("CommandFailed")
            },
        ),
        Err(error) => match error.downcast_ref::<CommandFailure>() {
            Some(error) => (error.returncode, error.stderr.as_deref(), Some(error.kind)),
            None => (None, None, Some("RustError")),
        },
    };
    let mut receipt = json!({"format_version": 1, "kind": "mysql-verification-output", "check": check, "target_key": database["key"], "returncode": code, "error_type": error_type, "stderr_bytes": stderr.map(<[u8]>::len), "stderr_sha256": stderr.map(|bytes| bytes_descriptor(bytes)["sha256"].clone()), "stdout_file": {"path": output.path()}});
    match snapshot {
        Ok(snapshot) => {
            receipt["stdout_file"] = snapshot.artifact.clone();
            if error_type.is_none() && std::str::from_utf8(&snapshot.raw).is_err() {
                receipt["error_type"] = json!("UnicodeDecodeError");
            }
        }
        Err(error) => {
            receipt["stdout_capture_error"] = json!(error.kind());
            if error_type.is_none() {
                receipt["error_type"] = json!(error.kind());
            }
        }
    }
    receipt
}

fn response(output: &CommandOutput) -> Result<String> {
    Ok(std::str::from_utf8(&output.stdout)
        .map_err(|_| "MySQL 核验输出不是 UTF-8")?
        .trim()
        .to_owned())
}

fn secrets(
    context: &StorageInputs<'_>,
    bound: Option<&Value>,
    side: CloneSide,
) -> Result<Vec<String>> {
    let mut result = context
        .environment
        .values()
        .filter(|value| !value.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    for db in context.databases()? {
        let connection = request_physical::defaults_connection(context.backend, &db)?;
        if let Some(password) = connection["password"]
            .as_str()
            .filter(|value| !value.is_empty())
        {
            result.push(password.into());
        }
    }
    for field in ["access_key", "secret_key"] {
        if let Some(value) = context.target()["s3"][field]
            .as_str()
            .filter(|s| !s.is_empty())
        {
            result.push(value.into());
        }
    }
    let configuration = super::binding::source_configuration(context, bound, side)?;
    let directory = Path::new(text(&configuration["directory"], "库存配置目录")?);
    let table =
        crate::rate_limit::load_app_table(&context.execution_backend()?, Some(directory), "test")
            .map_err(|_| "库存凭据配置无法安全读取")?;
    file_secrets(&serde_json::to_value(table)?, false, &mut result);
    if super::binding::source_configuration(context, bound, side)? != configuration {
        return Err("采集脱敏字段期间配置已变化".into());
    }
    result.sort_by_key(|value| std::cmp::Reverse(value.len()));
    result.dedup();
    Ok(result)
}

fn file_secrets(value: &Value, sensitive: bool, result: &mut Vec<String>) {
    match value {
        Value::String(value) if sensitive && !value.is_empty() => result.push(value.clone()),
        Value::Object(fields) => {
            for (key, value) in fields {
                let sensitive = sensitive
                    || matches!(
                        key.to_ascii_lowercase().as_str(),
                        "password"
                            | "secret"
                            | "secret_key"
                            | "access_key"
                            | "admin_password"
                            | "jwt_secret"
                            | "client_secret"
                            | "api_token"
                            | "token"
                            | "headers"
                            | "authorization"
                    );
                file_secrets(value, sensitive, result);
            }
        }
        Value::Array(values) => {
            for value in values {
                file_secrets(value, sensitive, result);
            }
        }
        _ => {}
    }
}

fn redact(bytes: &[u8], secrets: &[String]) -> String {
    let mut result = String::from_utf8_lossy(bytes).into_owned();
    for secret in secrets {
        result = result.replace(secret, "[REDACTED]");
    }
    result
}
