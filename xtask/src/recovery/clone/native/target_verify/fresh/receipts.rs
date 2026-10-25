//! 固定只读命令的完整证据；失败只发布安全原因，不回显工具 stderr。
use super::super::super::{
    export::transport::{CommandSpec, StdoutTarget, Transport},
    json as document,
};
use super::super::storage::StorageInputs;
use crate::Result;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

pub(super) fn command(
    inputs: &StorageInputs<'_>,
    transport: &impl Transport,
    program: PathBuf,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
    label: &str,
) -> Result<String> {
    let path = inputs.observation_dir.join(format!(
        "{label}-{}.command.json",
        uuid::Uuid::new_v4().simple()
    ));
    let command = std::iter::once(program.to_string_lossy().into_owned())
        .chain(args.clone())
        .collect::<Vec<_>>();
    let environment = command_environment(inputs);
    let secrets = command_secrets(inputs)?;
    let result = transport.run(CommandSpec {
        program,
        args,
        cwd: inputs.execution_backend()?,
        environment,
        stdin,
        stdout: StdoutTarget::Capture,
        // 传输原始 stdout/stderr 只留在已绑定私有 TEMP；失败材料不能混入公开观察树。
        work_dir: PathBuf::from(
            inputs
                .environment
                .get("TEMP")
                .ok_or("fresh 缺少已绑定 TEMP")?,
        ),
        label: label.into(),
        timeout: Duration::from_secs(30),
    });
    let (code, stdout, error) = match &result {
        Ok(output) if output.status.success() && !output.stderr.is_empty() => (
            output.status.code(),
            String::new(),
            Some("UnexpectedStderr"),
        ),
        Ok(output) if output.status.success() => match std::str::from_utf8(&output.stdout) {
            Ok(text) => (output.status.code(), text.to_owned(), None),
            Err(_) => (output.status.code(), String::new(), Some("InvalidUtf8")),
        },
        Ok(output) => (output.status.code(), String::new(), Some("CommandFailed")),
        Err(_) => (None, String::new(), Some("CommandFailed")),
    };
    // 成功 stdout 仅允许固定查询返回；失败原文可能包含私有连接信息，绝不写入公开证据。
    let diagnostic = super::super::super::maintenance::redact_with(&stdout, secrets);
    document::write_new(
        &path,
        &json!({"command":command,"returncode":code,"error_type":error,
        "stdout":diagnostic,"stderr":""}),
    )?;
    if error.is_some() {
        return Err("固定只读命令失败；核对私有文件配置、工具与服务，保留现场显式续作".into());
    }
    Ok(stdout)
}

fn command_environment(inputs: &StorageInputs<'_>) -> std::collections::BTreeMap<String, String> {
    let mut environment = std::env::vars()
        .filter(|(name, _)| {
            let key = name.to_ascii_uppercase();
            SYSTEM.contains(&key.as_str())
                || key.starts_with("PROCESSOR_")
                || key.starts_with("LC_")
        })
        .filter(|(key, _)| {
            !inputs
                .environment
                .keys()
                .any(|private| private.eq_ignore_ascii_case(key))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    environment.extend(inputs.environment.clone());
    environment.remove("MYSQL_PWD");
    environment.insert(
        "MYSQL_TEST_LOGIN_FILE".into(),
        inputs
            .observation_dir
            .join("unused-login.cnf")
            .to_string_lossy()
            .into_owned(),
    );
    environment
}

fn command_secrets(inputs: &StorageInputs<'_>) -> Result<Vec<String>> {
    let mut values = Vec::new();
    for database in document::array(&inputs.request["target"]["databases"], "fresh 四库")? {
        let connection =
            super::super::input::request_physical::defaults_connection(inputs.backend, database)?;
        values.push(document::text(&connection["password"], "MySQL 私有密码")?.to_owned());
    }
    for key in ["access_key", "secret_key"] {
        values
            .push(document::text(&inputs.request["target"]["s3"][key], "对象私有凭据")?.to_owned());
    }
    Ok(values)
}

pub(super) fn failure(stage: &str) -> Value {
    json!({"status":"needs_reconciliation","stage":stage,"at":chrono::Utc::now().to_rfc3339(),
        "error_type":"FreshPrepareError","reason":"只读 prepare 未完成；核对固定输入与完整现场后显式续作",
        "automatic_retry":false,"automatic_resource_cleanup":false,"fresh_target_initialized":false,
        "clone_verified":false,"restore_qualified":false})
}

// 与既有 configured 私有环境合并规则一致，只继承操作系统运行字段。
const SYSTEM: &[&str] = &[
    "APPDATA",
    "COMMONPROGRAMFILES",
    "COMMONPROGRAMFILES(X86)",
    "COMMONPROGRAMW6432",
    "COMSPEC",
    "HOME",
    "HOMEDRIVE",
    "HOMEPATH",
    "LANG",
    "LOCALAPPDATA",
    "NUMBER_OF_PROCESSORS",
    "OS",
    "PATH",
    "PATHEXT",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "SHELL",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
    "TMPDIR",
    "USER",
    "USERPROFILE",
    "WINDIR",
];
