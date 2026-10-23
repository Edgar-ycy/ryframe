use super::EvidenceDatabase;
use crate::{
    Result,
    devex::{
        job_evidence::protocol::Session,
        runtime_native::{mysql::Mysql, process::Cancellation},
    },
};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

pub(super) struct Database {
    session: Mysql,
    name: String,
}

impl Database {
    pub(super) fn open(table: &toml::Table) -> Result<Self> {
        let value = serde_json::to_value(table)?;
        let connection = value
            .pointer("/database/primary")
            .ok_or("配置缺少 database.primary")?;
        let text = |name| {
            connection[name]
                .as_str()
                .ok_or_else(|| format!("database.primary.{name} 缺失"))
        };
        let name = text("database")?.to_owned();
        if name.is_empty()
            || name.len() > 64
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err("database.primary.database 无效；请使用明确的隔离数据库名称".into());
        }
        let binding = json!({"control": {
            "host":text("host")?, "port":connection["port"], "database":name,
            "username":text("username")?, "password":text("password")?, "tls_mode":connection["tls_mode"].as_str().unwrap_or("required")
        }, "mysql_client": client()?});
        let session = Mysql::open(&binding, Cancellation::default(), Duration::from_secs(15))
            .map_err(|category| format!("隔离 MySQL 读取失败：{category}；检查绑定配置 database.primary、mysql 客户端和 TLS；没有业务写入"))?;
        let mut database = Self { session, name };
        database.query(&format!(
            "USE `{}`; SET SESSION TRANSACTION READ ONLY; START TRANSACTION READ ONLY",
            database.name
        ))?;
        Ok(database)
    }

    fn query(&mut self, sql: &str) -> Result<Vec<Value>> {
        self.session.query(sql).map_err(|category| format!("隔离 MySQL 只读查询失败：{category}；核对配置、权限、TLS 与数据库结构；禁止将失败视为对象已删除").into())
    }

    pub(super) fn identity(&mut self, scope: &str) -> Result<String> {
        let rows = self.query("SELECT JSON_OBJECT('server_uuid',@@server_uuid,'scope_id',scope_id,'marker',marker) FROM ryframe_resource_ownership WHERE resource_kind='control'")?;
        let [owner] = rows.as_slice() else {
            return Err("控制库 ownership 记录缺失或不唯一".into());
        };
        let server = owner["server_uuid"]
            .as_str()
            .ok_or("控制库缺少 server_uuid")?;
        if owner["scope_id"] != scope
            || owner["marker"] != format!("ryframe-owner:v1:{scope}:control")
            || !regex::Regex::new(r"^[a-f0-9-]{36}$")?.is_match(server)
        {
            return Err("控制库 ownership 与测试 scope 不匹配".into());
        }
        Ok(server.to_owned())
    }

    pub(super) fn close(&mut self) -> Result<()> {
        self.session.close().map_err(|category| {
            format!("MySQL 只读会话关闭失败：{category}；不能发布验收成功").into()
        })
    }
}

impl EvidenceDatabase for Database {
    fn record(&mut self, job_id: i64) -> Result<Value> {
        let rows = self.query(&format!("SELECT JSON_OBJECT('job_id',CAST(e.id AS CHAR),'file_id',CAST(f.id AS CHAR),'key',f.storage_path,'bucket',f.bucket,'sha256',f.file_sha256,'bytes',f.file_size) FROM sys_export_job e JOIN sys_file f ON f.id=e.result_file_id WHERE e.id={job_id} AND e.status='succeeded' AND e.delete_pending_at IS NULL"))?;
        let [row] = rows.as_slice() else {
            return Err("导出必须对应唯一的已完成文件".into());
        };
        Ok(row.clone())
    }
    fn remaining(&mut self, job_id: i64, file_id: i64) -> Result<u64> {
        let rows = self.query(&format!("SELECT (SELECT COUNT(*) FROM sys_export_job WHERE id={job_id}) + (SELECT COUNT(*) FROM sys_file WHERE id={file_id})"))?;
        match rows.as_slice() {
            [value] => value.as_u64().ok_or_else(|| "删除核验记录数无效".into()),
            _ => Err("删除核验必须返回唯一计数".into()),
        }
    }
    fn name(&self) -> &str {
        &self.name
    }
}

fn client() -> Result<PathBuf> {
    // PATH 仅定位第三方可执行文件；应用配置和凭据始终来自已绑定 TOML。
    let paths = std::env::var_os("PATH").ok_or("PATH 缺失；请安装 MySQL 客户端并加入 PATH")?;
    let name = if cfg!(windows) { "mysql.exe" } else { "mysql" };
    for path in std::env::split_paths(&paths).filter(|path| path.is_absolute()) {
        let candidate = path.join(name);
        if candidate.is_file() {
            return crate::local_test_path::validate_external_existing_file(&candidate)
                .map_err(Into::into);
        }
    }
    Err("未找到 MySQL 客户端；请安装与服务器版本匹配的 mysql 并加入 PATH，不使用环境凭据或容器包装器".into())
}
