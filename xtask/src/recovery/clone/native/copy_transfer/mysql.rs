//! 仅在写意图持久化后，用一条受管 MySQL 命令提交绑定的参考数据 SQL。
use std::{
    collections::BTreeSet,
    fs::{self, File, Metadata, OpenOptions},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use serde_json::{Value, json};

use super::super::{
    catalog, digest,
    export::transport::{CommandOutput, CommandSpec, StdoutTarget, Transport},
    input::digest_shape,
    json as document,
    resources::common,
    target_verify::{
        database::evidence::mysql_client_arguments,
        guard::Inputs,
        input::{request::external_file, request_physical::defaults_connection},
        publication,
        storage::StorageInputs,
    },
};
use crate::{
    Result,
    recovery::{
        inputs::domain,
        source::evidence::{
            reference::validate_dump_bytes, reference_workflow::readonly::defaults,
        },
    },
};

const SESSION_SQL: &str = concat!(
    "SET SESSION FOREIGN_KEY_CHECKS=0; SET SESSION time_zone='+00:00'; ",
    "SET SESSION SQL_MODE='NO_AUTO_VALUE_ON_ZERO'; START TRANSACTION;\n",
);

pub(crate) mod reference;

pub(crate) struct Write<'a> {
    pub(crate) copy_root: &'a Path,
    pub(crate) session_id: &'a str,
    pub(crate) export_root: &'a Path,
    pub(crate) item: &'a Value,
}

pub(crate) fn write(
    inputs: &StorageInputs<'_>,
    request: &Write<'_>,
    transport: &impl Transport,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<()> {
    checkpoint()?;
    common::scope_id(request.session_id)?;
    let copy = common::local_directory(inputs.backend, request.copy_root, "复制命令证据目录")?;
    let database = target_database(inputs, request.item)?;
    let source = Artifact::open(
        inputs.backend,
        request.export_root,
        &request.item["artifact"],
    )?;
    let payload = source.payload(request.item)?;
    let invocation = Invocation::prepare(inputs, &database, None)?;
    let spec = invocation.spec(inputs, payload)?;
    source.check()?;
    invocation.check(inputs)?;
    checkpoint()?;

    common::local_directory(inputs.backend, &copy, "复制命令证据目录")?;
    let path = copy.join(format!(
        "command-{}-{}.json",
        request.session_id,
        uuid::Uuid::new_v4().simple(),
    ));
    let command = std::iter::once(common::path_text(&spec.program, "MySQL 工具")?)
        .chain(spec.args.iter().cloned())
        .collect::<Vec<_>>();
    let receipt = json!({
        "format_version":1, "kind":"devex-copy-mysql-command", "command":command,
        "target_key":database["key"], "target_database":database["database"],
        "artifact":request.item["artifact"], "returncode":null, "error_type":null,
        "status":"pending", "remote_write_may_have_occurred":true,
        "automatic_retry_permitted":false, "output_policy":"status_only",
    });
    publication::publish(&path, &receipt)?;
    let binding = common::binding(&path)?;
    if document::read(&path)? != receipt {
        return Err("复制命令准备收据在调用前变化".into());
    }
    let (outcome, attempted, unchanged) =
        invoke(&source, &invocation, inputs, spec, transport, checkpoint);
    finish_command(
        inputs.backend,
        &path,
        &binding,
        receipt,
        outcome,
        attempted,
        unchanged,
    )
}

fn invoke(
    source: &Artifact<'_>,
    invocation: &Invocation<'_>,
    inputs: &StorageInputs<'_>,
    spec: CommandSpec,
    transport: &impl Transport,
    checkpoint: &dyn Fn() -> Result<()>,
) -> (Result<CommandOutput>, bool, bool) {
    let local = || source.check().and_then(|()| invocation.check(inputs));
    let before_launch = local().and_then(|()| checkpoint());
    let attempted = before_launch.is_ok();
    let outcome = before_launch.and_then(|()| transport.run_private(spec));
    let local = local();
    let external = checkpoint();
    (outcome, attempted, local.is_ok() && external.is_ok())
}

fn finish_command(
    backend: &Path,
    path: &Path,
    binding: &Value,
    mut receipt: Value,
    outcome: Result<CommandOutput>,
    attempted: bool,
    unchanged: bool,
) -> Result<()> {
    let passed = record_exit(&mut receipt, &outcome, attempted, unchanged);
    if common::bound_file(backend, binding, "复制命令准备收据")? != path {
        return Err("复制命令准备收据发生变化，写入结果需要核对".into());
    }
    document::write_replace(path, &receipt)?;
    if document::read(path)? != receipt {
        return Err("复制命令退出收据在发布后变化，写入结果需要核对".into());
    }
    if !passed {
        if !attempted {
            return Err("复制 MySQL 启动前证据发生变化；已保存未启动收据".into());
        }
        return Err("复制 MySQL 写入结果需要核对；已保留原退出状态，禁止自动重试".into());
    }
    Ok(())
}

fn record_exit(
    receipt: &mut Value,
    outcome: &Result<CommandOutput>,
    attempted: bool,
    unchanged: bool,
) -> bool {
    let exit = outcome.as_ref().ok().map(|output| output.status);
    let passed = exit.is_some_and(|status| status.success()) && unchanged;
    receipt["returncode"] = json!(exit.and_then(|status| status.code()));
    receipt["status"] = json!(if passed {
        "exited_zero"
    } else if attempted {
        "unknown"
    } else {
        "not_started"
    });
    receipt["remote_write_may_have_occurred"] = json!(attempted);
    receipt["error_type"] = json!(if !attempted {
        Some("CheckpointFailed")
    } else if outcome.is_err() {
        Some("CommandError")
    } else if exit.is_some_and(|status| !status.success()) {
        Some("CommandFailed")
    } else if !passed {
        Some("CheckpointFailed")
    } else {
        None
    });
    passed
}

struct Invocation<'a> {
    database: &'a Value,
    defaults: Inputs,
    work: PathBuf,
    work_id: (u64, u64),
    program: PathBuf,
    reference_tool: Option<(&'a Value, Inputs)>,
}

impl<'a> Invocation<'a> {
    fn prepare(
        inputs: &StorageInputs<'_>,
        database: &'a Value,
        reference_tool: Option<&'a Value>,
    ) -> Result<Self> {
        let defaults = Path::new(document::text(
            &database["defaults_file"],
            "MySQL 私有配置",
        )?);
        let defaults = common::common_file(inputs.backend, defaults, "MySQL 私有配置")?;
        let defaults = Inputs::open([defaults])?;
        let work = inputs
            .environment
            .get("TEMP")
            .ok_or("目标缺少已绑定私有 TEMP")?;
        let work = common::local_directory(inputs.backend, Path::new(work), "MySQL 私有 TEMP")?;
        let work_id = common::device_inode_for(&work, &fs::symlink_metadata(&work)?)?;
        let program = match reference_tool {
            Some(tool) => external_file(tool)?,
            None => inputs.tool("mysql")?,
        };
        let reference_tool = reference_tool
            .map(|tool| Inputs::open([program.clone()]).map(|held| (tool, held)))
            .transpose()?;
        Ok(Self {
            database,
            defaults,
            work,
            work_id,
            program,
            reference_tool,
        })
    }

    fn spec(&self, inputs: &StorageInputs<'_>, payload: Vec<u8>) -> Result<CommandSpec> {
        let mut environment = inputs.command_environment();
        environment.retain(|name, _| {
            !name.eq_ignore_ascii_case("MYSQL_PWD")
                && !name.eq_ignore_ascii_case("MYSQL_TEST_LOGIN_FILE")
        });
        environment.insert(
            "MYSQL_TEST_LOGIN_FILE".into(),
            common::path_text(
                &self.work.join("unused-login.cnf"),
                "禁用隐式 MySQL 登录配置",
            )?,
        );
        Ok(CommandSpec {
            program: self.program.clone(),
            args: mysql_client_arguments(self.database)?,
            cwd: inputs.execution_backend()?,
            environment,
            stdin: Some(payload),
            stdout: StdoutTarget::Capture,
            work_dir: self.work.clone(),
            label: "copy-mysql".into(),
            timeout: Duration::from_secs(1800),
        })
    }

    fn check(&self, inputs: &StorageInputs<'_>) -> Result<()> {
        self.defaults.check()?;
        match &self.reference_tool {
            Some((_, held)) => {
                held.check()?;
                let defaults = Path::new(document::text(
                    &self.database["defaults_file"],
                    "MySQL 私有配置",
                )?);
                if digest::file(defaults)?.sha256
                    != digest_shape(&self.database["defaults_sha256"])?
                {
                    return Err("参考恢复 MySQL 私有配置与计划摘要不同".into());
                }
                defaults::validate(defaults)?;
            }
            None => {
                defaults_connection(inputs.backend, self.database)?;
            }
        }
        let work = common::local_directory(inputs.backend, &self.work, "MySQL 私有 TEMP")?;
        if common::device_inode_for(&work, &fs::symlink_metadata(&work)?)? != self.work_id
            || match &self.reference_tool {
                Some((tool, _)) => external_file(tool)?,
                None => inputs.tool("mysql")?,
            } != self.program
        {
            return Err("目标 MySQL 工具或私有目录绑定变化".into());
        }
        match fs::symlink_metadata(work.join("unused-login.cnf")) {
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
            Ok(_) => Err("复制 MySQL 禁止读取隐式登录文件".into()),
        }
    }
}

fn target_database(inputs: &StorageInputs<'_>, item: &Value) -> Result<Value> {
    document::exact(
        item,
        &[
            "key",
            "source_database",
            "target_database",
            "tables",
            "artifact",
            "preserve_target_ownership",
        ],
        "复制数据库计划",
    )?;
    let key = document::text(&item["key"], "复制数据库 key")?;
    let database = document::text(&item["target_database"], "复制目标物理库")?;
    registered_database(inputs, key, database)
}

fn registered_database(inputs: &StorageInputs<'_>, key: &str, database: &str) -> Result<Value> {
    domain::identifier(key)?;
    domain::identifier(database)?;
    let mut matches = inputs
        .databases()?
        .into_iter()
        .filter(|db| db["key"].as_str() == Some(key));
    let selected = matches.next().ok_or("复制数据库未在目标请求登记")?;
    if matches.next().is_some() || selected["database"].as_str() != Some(database) {
        return Err("复制数据库必须匹配唯一登记的物理库".into());
    }
    Ok(selected)
}

struct Artifact<'a> {
    backend: &'a Path,
    root: PathBuf,
    relative: String,
    expected: digest::FileDigest,
    identity: same_file::Handle,
    modified: SystemTime,
    #[cfg(unix)]
    changed: (i64, i64),
    stream: File,
}

impl<'a> Artifact<'a> {
    fn open(backend: &'a Path, export_root: &Path, value: &Value) -> Result<Self> {
        document::exact(value, &["file", "bytes", "sha256"], "复制计划 SQL 产物")?;
        let relative = document::text(&value["file"], "复制计划 SQL 产物路径")?;
        Self::open_bound(backend, export_root, relative, value, None)
    }

    fn open_bound(
        backend: &'a Path,
        export_root: &Path,
        relative: &str,
        value: &Value,
        selected_path: Option<&Path>,
    ) -> Result<Self> {
        let root = common::local_directory(backend, export_root, "已绑定导出根目录")?;
        let path = digest::safe_file(&root, relative)?;
        if selected_path.is_some_and(|selected| selected != path) {
            return Err("参考恢复 SQL 必须绑定同一备份根内的登记产物".into());
        }
        let expected = digest::FileDigest {
            bytes: value["bytes"]
                .as_u64()
                .filter(|value| *value > 0 && *value < u64::MAX)
                .ok_or("复制计划 SQL 产物缺少有效读取上限")?,
            sha256: digest_shape(&value["sha256"])?.to_owned(),
        };
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1).custom_flags(0x0020_0000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
        }
        let stream = options.open(&path)?;
        let metadata = stream.metadata()?;
        let artifact = Self {
            backend,
            root,
            relative: relative.to_owned(),
            expected,
            identity: same_file::Handle::from_file(stream.try_clone()?)?,
            modified: metadata.modified()?,
            #[cfg(unix)]
            changed: unix_changed(&metadata),
            stream,
        };
        artifact.check()?;
        Ok(artifact)
    }

    fn check(&self) -> Result<()> {
        let root = common::local_directory(self.backend, &self.root, "已绑定导出根目录")?;
        let path = digest::safe_file(&root, &self.relative)?;
        let actual = fs::symlink_metadata(&path)?;
        if same_file::Handle::from_path(&path)? != self.identity
            || common::link_count_for(&path, &actual)? != 1
            || !self.unchanged(&actual)?
            || !self.unchanged(&self.stream.metadata()?)?
        {
            return Err("绑定的 SQL 产物身份、大小或修改状态变化".into());
        }
        Ok(())
    }

    fn unchanged(&self, metadata: &Metadata) -> Result<bool> {
        #[cfg(unix)]
        if unix_changed(metadata) != self.changed {
            return Ok(false);
        }
        Ok(metadata.is_file()
            && !digest::linked(metadata)
            && metadata.len() == self.expected.bytes
            && metadata.modified()? == self.modified)
    }

    fn payload(&self, item: &Value) -> Result<Vec<u8>> {
        let counts = document::object(&item["tables"], "复制计划业务表")?;
        let mut excluded = catalog::excluded_tables();
        excluded.extend(["seaql_tenant_data_migrations", "ryframe_business_migration"]);
        if item["preserve_target_ownership"].as_bool() != Some(true) || counts.is_empty() {
            return Err("复制 SQL 必须保留目标 ownership 并登记完整业务表".into());
        }
        let mut tables = BTreeSet::new();
        for (table, rows) in counts {
            domain::identifier(table)?;
            if rows.as_u64().is_none() || excluded.contains(&table.as_str()) {
                return Err("复制 SQL 表或行数没有取得计划授权".into());
            }
            tables.insert(table.clone());
        }

        self.ordered_payload(tables.iter().map(String::as_str), &tables)
    }

    fn ordered_payload<'b>(
        &self,
        order: impl IntoIterator<Item = &'b str>,
        allowed: &BTreeSet<String>,
    ) -> Result<Vec<u8>> {
        let mut payload = SESSION_SQL.as_bytes().to_vec();
        for table in order {
            payload.extend_from_slice(format!("DELETE FROM `{table}`;\n").as_bytes());
        }
        let start = payload.len();
        // 校验该缓冲区中的原字节，后续直接将同一缓冲区交给 stdin。
        (&self.stream)
            .take(self.expected.bytes + 1)
            .read_to_end(&mut payload)?;
        if validate_dump_bytes(&payload[start..], allowed)? != self.expected {
            return Err("SQL 转储与计划的原始产物摘要不同".into());
        }
        payload.extend_from_slice(b"\nCOMMIT;\n");
        Ok(payload)
    }
}

#[cfg(unix)]
fn unix_changed(metadata: &Metadata) -> (i64, i64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.ctime(), metadata.ctime_nsec())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/mysql.rs"]
mod tests;
