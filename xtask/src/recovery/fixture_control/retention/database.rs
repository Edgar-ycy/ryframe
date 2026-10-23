use super::super::{exact_object, read_json, text_field};
use crate::{
    Result,
    devex::{
        job_evidence::protocol::Session,
        runtime_native::{mysql::Mysql, process::Cancellation},
    },
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

const CONTAINER_CLIENT: &str = "umask 077\nconfig=$(mktemp) || exit 1\ntrap 'rm -f -- \"$config\"' EXIT\ntrap 'exit 130' HUP INT TERM\nIFS= read -r header && IFS= read -r credential || exit 1\nprintf '%s\\n' \"$header\" \"$credential\" > \"$config\" || exit 1\nclient=$1\nshift\n\"$client\" --defaults-file=\"$config\" \"$@\"\n";

pub(super) struct Connection {
    pub value: Value,
    pub schema: String,
    client: Client,
    options_root: PathBuf,
}
enum Client {
    Local(PathBuf),
    Container(String),
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/fixture_control/retention/database_tests.rs"]
mod tests;

pub(super) struct Invocation {
    pub command: Command,
    pub options: tempfile::NamedTempFile,
    pub prefix: Vec<u8>,
}

pub(super) fn bindings(root: &Path, table: &toml::Table, scope: &str) -> Result<[Connection; 3]> {
    selected(root, table, scope, &["dedicated-a", "dedicated-b"])?
        .try_into()
        .map_err(|_| "历史目标数量错误".into())
}

pub(super) fn gate_bindings(
    root: &Path,
    table: &toml::Table,
    scope: &str,
) -> Result<[Connection; 2]> {
    selected(root, table, scope, &["shared"])?
        .try_into()
        .map_err(|_| "复制门目标数量错误".into())
}

fn selected(
    root: &Path,
    table: &toml::Table,
    scope: &str,
    keys: &[&str],
) -> Result<Vec<Connection>> {
    let config = read_json(
        &root.join(".local-tests/full-stack-control.json"),
        "全栈工具配置",
        16 * 1024,
    )?;
    let fields = config.as_object().ok_or("全栈工具配置不是对象")?;
    let local = fields.contains_key("mysql_client");
    exact_object(
        &config,
        if local {
            &["format_version", "runtime_dir", "mysql_client"]
        } else {
            &[
                "format_version",
                "runtime_dir",
                "mysql_container_id",
                "redis_container_id",
                "s3_container_id",
            ]
        },
        "全栈工具配置",
    )?;
    if config["format_version"] != 1
        || !Path::new(text_field(&config, "runtime_dir", "全栈工具配置")?).is_absolute()
    {
        return Err("全栈工具配置版本或运行路径无效".into());
    }
    let value = serde_json::to_value(table)?;
    let targets = value
        .pointer("/tenant_data/targets")
        .and_then(Value::as_array)
        .ok_or("配置缺少 tenant_data.targets")?;
    let mut connections = Vec::new();
    for key in std::iter::once("control").chain(keys.iter().copied()) {
        let connection = if key == "control" {
            value
                .pointer("/database/primary")
                .ok_or("配置缺少 database.primary")?
                .clone()
        } else {
            let selected = targets
                .iter()
                .filter(|v| v["key"] == key)
                .collect::<Vec<_>>();
            let [target] = selected.as_slice() else {
                return Err("历史 fixture 必须有唯一专用目标".into());
            };
            if target["kind"] != "mysql"
                || target["mode"]
                    != if key == "shared" {
                        "shared"
                    } else {
                        "dedicated"
                    }
            {
                return Err("历史 fixture 目标必须是 MySQL dedicated".into());
            }
            (*target).clone()
        };
        validate(&connection)?;
        let client = client(&config, local, scope)?;
        connections.push(Connection {
            schema: text_field(&connection, "database", "连接")?.into(),
            value: connection,
            client,
            options_root: root.join(".local-tests/mysql-client"),
        });
    }
    let schemas = connections
        .iter()
        .map(|value| value.schema.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if schemas.len() != connections.len()
        || connections
            .iter()
            .any(|value| value.value["port"] != connections[0].value["port"])
    {
        return Err("迁移 fixture 必须使用同一 MySQL 实例的不同 schema".into());
    }
    Ok(connections)
}

fn client(config: &Value, local: bool, scope: &str) -> Result<Client> {
    Ok(if local {
        let path = PathBuf::from(text_field(config, "mysql_client", "全栈工具配置")?);
        validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
        Client::Local(path)
    } else {
        let container = text_field(config, "mysql_container_id", "全栈工具配置")?;
        if !(12..=64).contains(&container.len())
            || !container
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("MySQL 容器 ID 无效".into());
        }
        let run = std::env::var("GITHUB_RUN_ID").unwrap_or_default();
        let attempt = std::env::var("GITHUB_RUN_ATTEMPT").unwrap_or_default();
        if ![&run, &attempt]
            .iter()
            .all(|v| v.parse::<u64>().is_ok_and(|v| v > 0))
            || scope != format!("ci-{run}-{attempt}")
        {
            return Err("MySQL 容器必须明确属于当前 CI run 与 attempt".into());
        }
        Client::Container(container.into())
    })
}

fn validate(value: &Value) -> Result<()> {
    super::sql::identifier(text_field(value, "database", "连接")?)?;
    if !matches!(value["host"].as_str(), Some("127.0.0.1" | "localhost"))
        || value["port"]
            .as_u64()
            .is_none_or(|v| !(1..=65535).contains(&v))
    {
        return Err("迁移测试只允许明确的本机 MySQL 地址和有效端口".into());
    }
    if !matches!(
        value["tls_mode"].as_str(),
        Some("disabled" | "preferred" | "required")
    ) {
        return Err(
            "历史 gate 只支持 disabled、preferred、required TLS；不能静默忽略 CA 或主机名要求"
                .into(),
        );
    }
    for field in ["username", "password"] {
        let text = text_field(value, field, "MySQL 连接")?;
        if text.is_empty()
            || text.contains('\0')
            || text
                .chars()
                .any(|v| v.is_control() && !matches!(v, '\n' | '\r' | '\t' | '\u{8}'))
        {
            return Err(format!("MySQL {field} 无效；检查绑定明文文件").into());
        }
    }
    let certificate = value
        .get("tls_client_cert")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty());
    let key = value
        .get("tls_client_key")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty());
    if certificate.is_some() != key.is_some() {
        return Err("MySQL TLS 客户端证书与私钥必须成对配置".into());
    }
    for field in ["tls_ca", "tls_client_cert", "tls_client_key"] {
        if let Some(path) = value.get(field).filter(|value| !value.is_null()) {
            let path = path.as_str().ok_or("MySQL TLS 文件字段必须为文本路径")?;
            if !path.is_empty() {
                validate_absolute_path(Path::new(path), LocalTestPathKind::ExistingFile)?;
            }
        }
    }
    if value
        .as_object()
        .ok_or("连接不是对象")?
        .keys()
        .any(|v| v.ends_with("_env"))
        || value["username"]
            .as_str()
            .is_some_and(|s| s.starts_with('-'))
    {
        return Err("MySQL 连接不接受环境变量凭据或选项形式的用户名".into());
    }
    Ok(())
}

impl Connection {
    pub(super) fn invocation(&self, dump: bool) -> Result<Invocation> {
        validate(&self.value)?;
        if !self.options_root.exists() {
            fs::create_dir(&self.options_root)?;
        }
        validate_absolute_path(&self.options_root, LocalTestPathKind::ExistingDirectory)?;
        let mut options = tempfile::NamedTempFile::new_in(&self.options_root)?;
        let password = text_field(&self.value, "password", "MySQL 连接")?
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
            .replace('\u{8}', "\\b");
        let bytes = format!("[client]\npassword=\"{password}\"\n").into_bytes();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            options
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        options.write_all(&bytes)?;
        options.as_file().sync_all()?;
        let (mut command, port, prefix) = self.client_command(dump, options.path(), bytes)?;
        command.args([
            format!("--host={}", self.value["host"].as_str().ok_or("地址无效")?),
            format!("--port={port}"),
            format!(
                "--user={}",
                self.value["username"].as_str().ok_or("用户名无效")?
            ),
            "--protocol=TCP".into(),
            "--default-character-set=utf8mb4".into(),
            format!(
                "--ssl-mode={}",
                self.value["tls_mode"]
                    .as_str()
                    .ok_or("TLS 模式无效")?
                    .to_ascii_uppercase()
            ),
        ]);
        if !dump {
            command.args([
                "--batch",
                "--raw",
                "--unbuffered",
                "--skip-column-names",
                "--skip-reconnect",
                "--connect-timeout=5",
            ]);
        }
        for (field, flag) in [
            ("tls_ca", "--ssl-ca"),
            ("tls_client_cert", "--ssl-cert"),
            ("tls_client_key", "--ssl-key"),
        ] {
            if let Some(path) = self
                .value
                .get(field)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            {
                command.arg(format!("{flag}={path}"));
            }
        }
        child_environment(&mut command);
        Ok(Invocation {
            command,
            options,
            prefix,
        })
    }

    fn client_command(
        &self,
        dump: bool,
        options: &Path,
        bytes: Vec<u8>,
    ) -> Result<(Command, u64, Vec<u8>)> {
        Ok(match &self.client {
            Client::Local(path) => {
                let path = if dump {
                    path.with_file_name(if cfg!(windows) {
                        "mysqldump.exe"
                    } else {
                        "mysqldump"
                    })
                } else {
                    path.clone()
                };
                validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
                let mut command = Command::new(path);
                command.arg(format!("--defaults-file={}", options.display()));
                (
                    command,
                    self.value["port"].as_u64().ok_or("端口无效")?,
                    Vec::new(),
                )
            }
            Client::Container(container) => {
                let mut command = Command::new("docker");
                command.args([
                    "exec",
                    "-i",
                    container,
                    "sh",
                    "-c",
                    CONTAINER_CLIENT,
                    "ryframe-mysql",
                    if dump { "mysqldump" } else { "mysql" },
                ]);
                (command, 3306, bytes)
            }
        })
    }

    pub(super) fn open(&self) -> Result<Mysql> {
        let invocation = self.invocation(false)?;
        Mysql::open_command(
            invocation.command,
            invocation.options,
            &invocation.prefix,
            Cancellation::default(),
            Duration::from_secs(15),
        )
        .map_err(|category| {
            format!(
                "迁移 MySQL 会话失败：{category}；核对隔离配置、客户端、TLS 和权限；禁止自动重放"
            )
            .into()
        })
    }
}

fn child_environment(command: &mut Command) {
    // 只继承操作系统和第三方客户端运行环境；凭据不进入环境或诊断。
    let keep = std::env::vars_os()
        .filter(|(key, _)| {
            matches!(
                key.to_string_lossy().to_ascii_uppercase().as_str(),
                "PATH"
                    | "PATHEXT"
                    | "SYSTEMROOT"
                    | "WINDIR"
                    | "COMSPEC"
                    | "HOME"
                    | "USERPROFILE"
                    | "HOMEDRIVE"
                    | "HOMEPATH"
                    | "TEMP"
                    | "TMP"
                    | "DOCKER_HOST"
                    | "DOCKER_CONFIG"
                    | "DOCKER_TLS_VERIFY"
                    | "DOCKER_CERT_PATH"
            )
        })
        .collect::<Vec<_>>();
    command
        .env_clear()
        .envs(keep)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
}

pub(super) fn query(session: &mut impl Session, sql: &str) -> Result<Vec<Value>> {
    session.query(sql).map_err(|category| {
        format!("迁移 MySQL 确认失败：{category}；保留前后像，写入结果可能未知，禁止自动重放")
            .into()
    })
}

pub(super) fn ownership(
    session: &mut impl Session,
    schema: &str,
    scope: &str,
    kind: &str,
) -> Result<String> {
    super::sql::identifier(schema)?;
    let rows = query(
        session,
        &format!(
            "SELECT JSON_OBJECT('scope',scope_id,'marker',marker,'server',@@server_uuid) FROM `{schema}`.ryframe_resource_ownership WHERE resource_kind='{kind}'"
        ),
    )?;
    let [row] = rows.as_slice() else {
        return Err("MySQL ownership 缺失或不唯一".into());
    };
    if row["scope"] != scope || row["marker"] != format!("ryframe-owner:v1:{scope}:{kind}") {
        return Err("MySQL scope 或 ownership marker 不匹配".into());
    }
    Ok(text_field(row, "server", "MySQL ownership")?.to_owned())
}
