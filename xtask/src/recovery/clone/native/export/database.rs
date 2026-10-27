use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Value, json};

use super::super::{
    catalog::{self, Catalog},
    digest, json as json_io, sql,
};
use super::{
    input::Context,
    transport::{CommandSpec, StdoutTarget, Transport, redact},
};
use crate::Result;

pub(crate) struct Models {
    pub(crate) control: Catalog,
    pub(crate) tenant: Catalog,
    pub(crate) full_control: Catalog,
    pub(crate) full_tenant: Catalog,
}

pub(crate) fn models(context: &Context) -> Result<Models> {
    let loaded = catalog::load(&context.execution_backend)?;
    let full_control = catalog::parse_schema(&std::fs::read_to_string(
        context.execution_backend.join("sql/ryframe_config.sql"),
    )?)?;
    let mut full_tenant =
        catalog::parse_schema(&std::fs::read_to_string(context.execution_backend.join(
            "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
        ))?)?;
    full_tenant.extend(loaded.tenant.clone());
    Ok(Models {
        control: loaded.control,
        tenant: loaded.tenant,
        full_control,
        full_tenant,
    })
}

pub(crate) fn verify_migrations<T: Transport>(
    context: &Context,
    transport: &T,
    phase: &str,
) -> Result<()> {
    let executable = context.maintenance_executable("migrate")?;
    run_logged(
        context,
        transport,
        &format!("migrate-{phase}-control"),
        &executable,
        &["control".into(), "verify".into()],
        None,
    )?;
    for database in context.databases()? {
        let key = database["key"].as_str().ok_or("数据库 key 无效")?;
        run_logged(
            context,
            transport,
            &format!("migrate-{phase}-target-{key}"),
            &executable,
            &[
                "tenant-data".into(),
                "verify".into(),
                "--target".into(),
                key.into(),
            ],
            None,
        )?;
    }
    Ok(())
}

pub(crate) fn schema_snapshot<T: Transport>(
    context: &Context,
    models: &Models,
    transport: &T,
    phase: &str,
) -> Result<Vec<Value>> {
    let output = context.output();
    let mut result = Vec::new();
    for database in context.databases()? {
        let key = database["key"].as_str().ok_or("数据库 key 无效")?;
        let mut declared = if database["kind"].as_str() == Some("combined") {
            models.full_control.clone()
        } else {
            Catalog::new()
        };
        declared.extend(models.full_tenant.clone());
        let expected_tables = declared.keys().cloned().collect::<BTreeSet<_>>();
        let response = mysql(
            context,
            transport,
            &database,
            "SELECT TABLE_NAME, COLUMN_NAME, UPPER(DATA_TYPE) FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME, ORDINAL_POSITION;",
            "源 schema 查询",
        )?;
        let mut actual: Catalog = Catalog::new();
        for line in response.lines() {
            let mut parts = line.split('\t');
            let Some(table) = parts.next() else {
                return Err("源 schema 列清单为空".into());
            };
            let Some(column) = parts.next() else {
                return Err("源 schema 列清单缺少列名".into());
            };
            let Some(kind) = parts.next() else {
                return Err("源 schema 列清单缺少类型".into());
            };
            if parts.next().is_some()
                || actual
                    .entry(table.to_owned())
                    .or_default()
                    .insert(column.to_owned(), kind.to_owned())
                    .is_some()
            {
                return Err("源 schema 列清单无效或重复".into());
            }
        }
        let mut expected_with_ledgers = expected_tables;
        expected_with_ledgers.insert("seaql_tenant_data_migrations".into());
        if database["kind"].as_str() == Some("combined") {
            expected_with_ledgers.insert("seaql_migrations".into());
        }
        if actual.keys().cloned().collect::<BTreeSet<_>>() != expected_with_ledgers
            || declared
                .iter()
                .any(|(table, columns)| actual.get(table) != Some(columns))
        {
            return Err("源完整表或列与当前 schema catalog 不符，不能过滤未知表".into());
        }
        let path = output.join(format!("schema-{phase}-{key}.json"));
        json_io::write_new(&path, &json!({"key": key, "columns": actual}))?;
        result.push(json!({
            "key": key,
            "columns": actual,
            "sha256": digest::plan_hash(&json!(actual))?,
        }));
    }
    Ok(result)
}

pub(crate) fn dump<T: Transport>(
    context: &Context,
    transport: &T,
    database: &Value,
    models: &Models,
) -> Result<(BTreeMap<String, u64>, Value)> {
    let key = database["key"].as_str().ok_or("数据库 key 无效")?;
    let combined = database["kind"].as_str() == Some("combined");
    let catalog = sql::all_columns(&models.control, &models.tenant, combined);
    let directory = context.output().join("databases");
    fs::create_dir(&directory).map_err(|error| format!("无法创建数据库转储目录：{error}"))?;
    let raw = directory.join(format!("{key}.raw.sql"));
    let output = directory.join(format!("{key}.sql"));
    let defaults = validate_defaults(Path::new(
        database
            .get("defaults_file")
            .and_then(Value::as_str)
            .ok_or("数据库缺少凭据文件")?,
    ))?;
    let program = context.tool("mysqldump")?;
    let database_name = database
        .get("database")
        .and_then(Value::as_str)
        .ok_or("数据库名称无效")?;
    let mut args = vec![
        format!("--defaults-file={}", defaults.display()),
        "--no-create-info".into(),
        "--skip-triggers".into(),
        "--skip-lock-tables".into(),
        "--skip-add-locks".into(),
        "--skip-comments".into(),
        "--skip-disable-keys".into(),
        "--skip-extended-insert".into(),
        "--complete-insert".into(),
        "--hex-blob".into(),
        "--set-gtid-purged=OFF".into(),
        "--no-tablespaces".into(),
        "--default-character-set=utf8mb4".into(),
        database_name.into(),
    ];
    args.extend(catalog.keys().cloned());
    transport
        .run(command(
            context,
            program,
            args,
            StdoutTarget::File(raw.clone()),
            None,
            "源数据库转储",
        ))?
        .success("源数据库转储")?;
    normalize_dump(&raw, &output, &catalog)?;
    let _ = fs::remove_file(&raw);
    let counts = validate_dump(&output, &catalog)?;
    Ok((
        counts,
        json!({
            "file": format!("databases/{key}.sql"),
            "bytes": digest::file(&output)?.bytes,
            "sha256": digest::file(&output)?.sha256,
        }),
    ))
}

pub(crate) fn mysql<T: Transport>(
    context: &Context,
    transport: &T,
    database: &Value,
    sql: &str,
    label: &str,
) -> Result<String> {
    let defaults = validate_defaults(Path::new(
        database
            .get("defaults_file")
            .and_then(Value::as_str)
            .ok_or("数据库缺少凭据文件")?,
    ))?;
    let database_name = database
        .get("database")
        .and_then(Value::as_str)
        .ok_or("数据库名称无效")?;
    let args = vec![
        format!("--defaults-file={}", defaults.display()),
        format!("--database={database_name}"),
        "--batch".into(),
        "--raw".into(),
        "--skip-column-names".into(),
        "--binary-mode".into(),
        "--default-character-set=utf8mb4".into(),
        "--execute".into(),
        sql.into(),
    ];
    let output = transport.run(command(
        context,
        context.tool("mysql")?,
        args,
        StdoutTarget::Capture,
        None,
        label,
    ))?;
    let output = output.success(label)?;
    Ok(output.utf8(label)?.trim().to_owned())
}

pub(crate) fn run_logged<T: Transport>(
    context: &Context,
    transport: &T,
    stage: &str,
    program: &Path,
    args: &[String],
    stdin: Option<Vec<u8>>,
) -> Result<()> {
    let work = context.output();
    let configuration = &context.source()["s3"]["configuration"];
    super::super::s3::app_configuration::private_configuration(&context.backend, configuration)?;
    let directory = json_io::text(&configuration["directory"], "来源CLI绑定配置目录")?;
    let mut actual_args = vec!["--config-dir".to_owned(), directory.to_owned()];
    actual_args.extend_from_slice(args);
    let command_file = work.join(format!("{stage}.command.json"));
    json_io::write_new(
        &command_file,
        &json!({
            "command": actual_args,
            "cwd": context.execution_backend,
            "remote_operations": "read_only",
        }),
    )?;
    let diagnostic = work.join(format!("{stage}.diagnostic.json"));
    let result = transport.run(CommandSpec {
        program: program.to_path_buf(),
        args: actual_args,
        cwd: context.execution_backend.clone(),
        environment: mysql_environment(context),
        stdin,
        stdout: StdoutTarget::Capture,
        work_dir: work,
        label: stage.to_owned(),
        timeout: Duration::from_secs(1800),
    });
    match result {
        Ok(output) if output.status.success() => {
            json_io::write_new(
                &diagnostic,
                &json!({"returncode": 0, "error_type": null, "stdout": "", "stderr": ""}),
            )?;
            Ok(())
        }
        Ok(output) => {
            let stderr = redact(
                &String::from_utf8_lossy(&output.stderr),
                &BTreeMap::from_iter(
                    context
                        .environment
                        .iter()
                        .filter(|(key, _)| key.contains("PASSWORD") || key.contains("SECRET"))
                        .map(|(key, value)| (key.clone(), value.clone())),
                ),
            );
            json_io::write_new(
                &diagnostic,
                &json!({"returncode": output.status.code(), "error_type": "CommandFailed", "stdout": "", "stderr": stderr}),
            )?;
            Err(format!("{stage} 命令失败：{}", output.status).into())
        }
        Err(error) => {
            json_io::write_new(
                &diagnostic,
                &json!({"returncode": null, "error_type": "CommandError", "stdout": "", "stderr": error.to_string()}),
            )?;
            Err(error)
        }
    }
}

fn command(
    context: &Context,
    program: PathBuf,
    args: Vec<String>,
    stdout: StdoutTarget,
    stdin: Option<Vec<u8>>,
    label: &str,
) -> CommandSpec {
    CommandSpec {
        program,
        args,
        cwd: context.execution_backend.clone(),
        environment: mysql_environment(context),
        stdin,
        stdout,
        work_dir: context.output(),
        label: label.to_owned(),
        timeout: Duration::from_secs(1800),
    }
}

fn mysql_environment(context: &Context) -> BTreeMap<String, String> {
    let mut environment = context.environment.clone();
    environment.remove("MYSQL_PWD");
    environment.insert(
        "MYSQL_TEST_LOGIN_FILE".into(),
        context
            .output()
            .join("unused-login.cnf")
            .display()
            .to_string(),
    );
    environment
}

fn validate_defaults(path: &Path) -> Result<PathBuf> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("无法读取 MySQL 凭据文件 {}：{error}", path.display()))?;
    let mut section = String::new();
    let mut values = BTreeMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_owned();
            continue;
        }
        if section != "client" {
            return Err("MySQL 凭据文件只允许 client 段".into());
        }
        let (key, value) = line.split_once('=').ok_or("MySQL 凭据文件字段格式无效")?;
        let key = key.trim().to_ascii_lowercase();
        if !matches!(
            key.as_str(),
            "host" | "port" | "user" | "password" | "ssl-mode" | "ssl-ca" | "ssl-cert" | "ssl-key"
        ) || values.insert(key, value.trim().to_owned()).is_some()
        {
            return Err("MySQL 凭据文件包含未允许、重复或越界的字段".into());
        }
    }
    if !matches!(
        values.get("host").map(String::as_str),
        Some("127.0.0.1" | "::1")
    ) || values.get("user").is_none_or(String::is_empty)
        || !values.contains_key("password")
        || values
            .get("port")
            .and_then(|value| value.parse::<u16>().ok())
            .is_none_or(|value| value == 0)
    {
        return Err("MySQL 凭据文件只允许本机连接、认证和 TLS 字段".into());
    }
    Ok(path.to_path_buf())
}

fn normalize_dump(raw: &Path, output: &Path, catalog: &Catalog) -> Result<()> {
    let source = fs::read_to_string(raw)?;
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|error| format!("无法创建规范化 SQL 转储：{error}"))?;
    use std::io::Write;
    writeln!(target, "-- ryframe-reference-data-v1")?;
    for line in source.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() || line.starts_with("/*!") && line.ends_with("*/;") {
            continue;
        }
        sql::parse_row(line, catalog)?;
        writeln!(target, "{line}")?;
    }
    target.sync_all()?;
    Ok(())
}

fn validate_dump(path: &Path, catalog: &Catalog) -> Result<BTreeMap<String, u64>> {
    let mut counts = sql::empty_counts(catalog);
    sql::for_each_row(path, catalog, None, |table, _| {
        *counts.get_mut(table).ok_or("转储包含未知表")? += 1;
        Ok(())
    })?;
    Ok(counts)
}
