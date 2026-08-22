//! 离线代码生成器的命令行参数契约。

use std::{collections::HashSet, fmt};

use crate::{GenerateOptions, normalize_relative_output_path, validate_table_name};

const DATABASE_URL_ENV: &str = "RYFRAME_GEN_DATABASE_URL";
const FALLBACK_DATABASE_URL_ENV: &str = "DATABASE_URL";
const TABLES_ENV: &str = "RYFRAME_GEN_TABLES";

pub struct SecretDatabaseUrl {
    value: String,
    target: String,
}

impl SecretDatabaseUrl {
    pub fn new(value: String) -> Result<Self, CliError> {
        if value.trim() != value {
            return Err(CliError::new(
                "数据库 URL 首尾不能包含空白，且错误信息不会回显原始 URL",
            ));
        }
        let target = sanitized_database_target(&value)?;
        Ok(Self { value, target })
    }

    pub fn expose(&self) -> &str {
        &self.value
    }

    pub fn target(&self) -> &str {
        &self.target
    }
}

impl fmt::Debug for SecretDatabaseUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SecretDatabaseUrl")
            .field(&self.target)
            .finish()
    }
}

#[derive(Default)]
pub struct CliEnvironment {
    pub database_url: Option<String>,
    pub tables: Option<String>,
}

impl CliEnvironment {
    pub fn from_process() -> Self {
        Self {
            database_url: read_environment(DATABASE_URL_ENV)
                .or_else(|| read_environment(FALLBACK_DATABASE_URL_ENV)),
            tables: read_environment(TABLES_ENV),
        }
    }
}

pub struct CliOptions {
    pub database_url: SecretDatabaseUrl,
    pub generate: GenerateOptions,
    pub write: bool,
}

#[derive(Default)]
struct RawOptions {
    database_url: Option<String>,
    tables: Option<String>,
    table_prefixes: Option<String>,
    entity_dir: Option<String>,
    repository_dir: Option<String>,
    use_case_dir: Option<String>,
    handler_dir: Option<String>,
    dto_dir: Option<String>,
    comments: bool,
    write: bool,
    overwrite: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CliError(String);

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

pub fn parse_cli<I>(
    arguments: I,
    environment: CliEnvironment,
) -> Result<Option<CliOptions>, CliError>
where
    I: IntoIterator<Item = String>,
{
    let mut arguments = arguments.into_iter();
    let mut raw = RawOptions::default();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-h" | "--help" => return Ok(None),
            "--database-url" => set_value(
                &mut raw.database_url,
                next_value(&mut arguments, "--database-url")?,
                "--database-url",
            )?,
            "--tables" => set_value(
                &mut raw.tables,
                next_value(&mut arguments, "--tables")?,
                "--tables",
            )?,
            "--table-prefixes" => set_value(
                &mut raw.table_prefixes,
                next_value(&mut arguments, "--table-prefixes")?,
                "--table-prefixes",
            )?,
            "--entity-dir" => set_value(
                &mut raw.entity_dir,
                next_value(&mut arguments, "--entity-dir")?,
                "--entity-dir",
            )?,
            "--repository-dir" => set_value(
                &mut raw.repository_dir,
                next_value(&mut arguments, "--repository-dir")?,
                "--repository-dir",
            )?,
            "--use-case-dir" => set_value(
                &mut raw.use_case_dir,
                next_value(&mut arguments, "--use-case-dir")?,
                "--use-case-dir",
            )?,
            "--handler-dir" => set_value(
                &mut raw.handler_dir,
                next_value(&mut arguments, "--handler-dir")?,
                "--handler-dir",
            )?,
            "--dto-dir" => set_value(
                &mut raw.dto_dir,
                next_value(&mut arguments, "--dto-dir")?,
                "--dto-dir",
            )?,
            "--comments" => set_flag(&mut raw.comments, "--comments")?,
            "--write" => set_flag(&mut raw.write, "--write")?,
            "--overwrite" => set_flag(&mut raw.overwrite, "--overwrite")?,
            _ => {
                return Err(CliError::new(
                    "存在未知参数；为避免泄露参数内容，错误信息不会回显原始值",
                ));
            }
        }
    }

    if raw.overwrite && !raw.write {
        return Err(CliError::new("`--overwrite` 必须与 `--write` 同时使用"));
    }

    let database_url = raw
        .database_url
        .take()
        .or(environment.database_url)
        .ok_or_else(|| {
            CliError::new(format!(
                "缺少数据库 URL；请使用 `--database-url`、`{DATABASE_URL_ENV}` 或 `{FALLBACK_DATABASE_URL_ENV}`"
            ))
        })?;
    let tables =
        raw.tables.take().or(environment.tables).ok_or_else(|| {
            CliError::new(format!("缺少数据表；请使用 `--tables` 或 `{TABLES_ENV}`"))
        })?;

    let mut generate = GenerateOptions {
        tables: parse_table_names(&tables)?,
        table_prefixes: raw
            .table_prefixes
            .as_deref()
            .map(parse_table_prefixes)
            .transpose()?
            .unwrap_or_default(),
        generate_comments: raw.comments,
        overwrite: raw.overwrite,
        ..GenerateOptions::default()
    };
    apply_output_paths(&mut generate, &raw)?;

    Ok(Some(CliOptions {
        database_url: SecretDatabaseUrl::new(database_url)?,
        generate,
        write: raw.write,
    }))
}

fn apply_output_paths(options: &mut GenerateOptions, raw: &RawOptions) -> Result<(), CliError> {
    options.entity_dir = normalized_output_path(
        raw.entity_dir.as_deref().unwrap_or(&options.entity_dir),
        "实体输出目录",
    )?;
    options.repository_dir = normalized_output_path(
        raw.repository_dir
            .as_deref()
            .unwrap_or(&options.repository_dir),
        "Repository 输出目录",
    )?;
    options.use_case_dir = normalized_output_path(
        raw.use_case_dir.as_deref().unwrap_or(&options.use_case_dir),
        "应用用例输出目录",
    )?;
    options.handler_dir = normalized_output_path(
        raw.handler_dir.as_deref().unwrap_or(&options.handler_dir),
        "Handler 输出目录",
    )?;
    options.dto_dir = normalized_output_path(
        raw.dto_dir.as_deref().unwrap_or(&options.dto_dir),
        "DTO 输出目录",
    )?;
    Ok(())
}

fn normalized_output_path(path: &str, label: &str) -> Result<String, CliError> {
    normalize_relative_output_path(path, label).map_err(|error| CliError::new(error.to_string()))
}

fn next_value<I>(arguments: &mut I, flag: &str) -> Result<String, CliError>
where
    I: Iterator<Item = String>,
{
    let value = arguments
        .next()
        .ok_or_else(|| CliError::new(format!("`{flag}` 缺少参数值")))?;
    if value.starts_with('-') || value.trim().is_empty() {
        return Err(CliError::new(format!("`{flag}` 缺少有效参数值")));
    }
    Ok(value)
}

fn set_value(slot: &mut Option<String>, value: String, flag: &str) -> Result<(), CliError> {
    if slot.replace(value).is_some() {
        return Err(CliError::new(format!("`{flag}` 不能重复指定")));
    }
    Ok(())
}

fn set_flag(slot: &mut bool, flag: &str) -> Result<(), CliError> {
    if *slot {
        return Err(CliError::new(format!("`{flag}` 不能重复指定")));
    }
    *slot = true;
    Ok(())
}

fn parse_table_names(value: &str) -> Result<Vec<String>, CliError> {
    let names = parse_comma_separated(value, "数据表")?;
    let mut unique = HashSet::with_capacity(names.len());
    for name in &names {
        validate_table_name(name).map_err(|_| {
            CliError::new("数据表名称只能包含字母、数字和下划线，且不能包含路径片段")
        })?;
        if !unique.insert(name.as_str()) {
            return Err(CliError::new("数据表名称不能重复"));
        }
    }
    Ok(names)
}

fn parse_table_prefixes(value: &str) -> Result<Vec<String>, CliError> {
    let prefixes = parse_comma_separated(value, "表名前缀")?;
    if prefixes.iter().any(|prefix| {
        !prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }) {
        return Err(CliError::new("表名前缀只能包含字母、数字和下划线"));
    }
    Ok(prefixes)
}

fn parse_comma_separated(value: &str, label: &str) -> Result<Vec<String>, CliError> {
    let values = value
        .split(',')
        .map(str::trim)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if values.is_empty() || values.iter().any(String::is_empty) {
        return Err(CliError::new(format!(
            "{label}不能为空，多个值请使用英文逗号分隔"
        )));
    }
    Ok(values)
}

fn sanitized_database_target(value: &str) -> Result<String, CliError> {
    let rest = value.strip_prefix("mysql://").ok_or_else(|| {
        CliError::new("数据库 URL 必须使用 `mysql://`，且错误信息不会回显原始 URL")
    })?;
    let without_fragment = rest.split('#').next().unwrap_or_default();
    let without_query = without_fragment.split('?').next().unwrap_or_default();
    let (authority, path) = without_query
        .split_once('/')
        .ok_or_else(|| CliError::new("数据库 URL 必须包含主机和数据库名，且不会回显凭据"))?;
    let endpoint = authority
        .rsplit_once('@')
        .map_or(authority, |(_, endpoint)| endpoint);
    let database = path.split('/').next().unwrap_or_default();

    let endpoint_is_safe = !endpoint.is_empty()
        && endpoint.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        });
    let database_is_safe = !database.is_empty()
        && database
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'$'));
    if !endpoint_is_safe || !database_is_safe {
        return Err(CliError::new(
            "数据库 URL 的主机或数据库名格式无效，且不会回显凭据",
        ));
    }

    Ok(format!("mysql://{endpoint}/{database}"))
}

fn read_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub fn help_text() -> String {
    let defaults = GenerateOptions::default();
    format!(
        r#"RyFrame 离线代码生成器

用法：
  cargo run -p ryframe-generator --bin ryframe-gen -- [选项]

必需输入（命令行优先于环境变量）：
  --database-url <URL>       MySQL URL；也可使用 {DATABASE_URL_ENV} 或 {FALLBACK_DATABASE_URL_ENV}
  --tables <表1,表2>         逗号分隔的数据表；也可使用 {TABLES_ENV}

安全写入：
  默认                         dry-run，只列出生成文件，不写盘
  --write                    显式写入当前工作区
  --overwrite                覆盖已有文件；必须与 --write 同时使用

生成选项：
  --table-prefixes <前缀,...> 去除表名前缀
  --comments                 生成数据库注释
  --entity-dir <路径>        默认：{entity_dir}
  --repository-dir <路径>    默认：{repository_dir}
  --use-case-dir <路径>      默认：{use_case_dir}
  --handler-dir <路径>       默认：{handler_dir}
  --dto-dir <路径>           默认：{dto_dir}

所有输出目录必须是工作区相对路径，禁止绝对路径、空片段、`.` 与 `..`。
实体和 Repository 只能生成到 ryframe-db，应用用例只能生成到 ryframe-application，Handler 和 DTO 只能生成到 ryframe-api。
Repository 只接收连接或事务；事务边界由应用用例控制。

其他：
  -h, --help                 显示帮助
"#,
        entity_dir = defaults.entity_dir,
        repository_dir = defaults.repository_dir,
        use_case_dir = defaults.use_case_dir,
        handler_dir = defaults.handler_dir,
        dto_dir = defaults.dto_dir,
    )
}
