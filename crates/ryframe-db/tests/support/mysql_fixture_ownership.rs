use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use same_file::Handle;
use sea_orm::{
    DatabaseConnection,
    sqlx::{self, MySql, Row, pool::PoolConnection},
};
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    pub server_uuid: String,
    pub connection_id: u64,
}

pub(super) fn business_identity_matches(
    server_uuid: &str,
    schema: &str,
    observed_uuid: &str,
    observed_schema: Option<&str>,
) -> bool {
    server_uuid == observed_uuid && observed_schema == Some(schema)
}

#[derive(Clone, Debug)]
pub(super) struct Plan {
    pub schema: String,
    pub scope: String,
    pub test: String,
    pub identity: Identity,
}

impl Plan {
    pub fn validate(&self) -> Result<(), String> {
        validate_schema(&self.schema)?;
        if self.identity.connection_id == 0 || self.identity.server_uuid.is_empty() {
            return Err("fixture 管理连接身份无效".into());
        }
        Ok(())
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let value = json!({"format_version": 1, "schema": self.schema,
            "scope": self.scope, "test": self.test,
            "server_uuid": self.identity.server_uuid,
            "connection_id": self.identity.connection_id,
            "lock_key": self.schema, "lock_owner": self.identity.connection_id,
            "phase": "planned"});
        let mut bytes = serde_json::to_vec_pretty(&value).map_err(|_| "无法编码 fixture 计划")?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

pub(super) fn validate_schema(schema: &str) -> Result<(), String> {
    if schema.is_empty()
        || schema.len() > 64
        || !schema
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("fixture schema 必须为 1 到 64 字节 ASCII 字母、数字或下划线".into());
    }
    Ok(())
}

fn permits_processlist(grant: &str) -> bool {
    grant
        .strip_prefix("GRANT ")
        .and_then(|value| value.split_once(" ON *.* TO "))
        .is_some_and(|(privileges, _)| {
            privileges == "ALL PRIVILEGES"
                || privileges
                    .split(", ")
                    .any(|privilege| privilege == "PROCESS")
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Planned,
    CreateIntent,
    Created,
    TestPassed,
    DropIntent,
    Dropped,
    Retained,
}

pub(super) struct Ownership {
    pub plan: Plan,
    phase: Phase,
}

#[derive(Clone)]
pub(super) struct Observation {
    pub identity: Identity,
    pub lock_owner: Option<u64>,
    pub plan_identity_matches: bool,
    pub plan_content_matches: bool,
    pub journal_matches: bool,
    pub business_closed: bool,
    pub schema_exists: bool,
    pub process_visibility: bool,
    pub active_connections: u64,
}

impl Ownership {
    pub fn new(plan: Plan) -> Result<Self, String> {
        plan.validate()?;
        Ok(Self {
            plan,
            phase: Phase::Planned,
        })
    }

    pub fn create_intent(&mut self) -> Result<(), String> {
        self.transition(Phase::Planned, Phase::CreateIntent)
    }

    pub fn created(&mut self) -> Result<(), String> {
        self.transition(Phase::CreateIntent, Phase::Created)
    }

    pub fn test_passed(&mut self) -> Result<(), String> {
        self.transition(Phase::Created, Phase::TestPassed)
    }

    fn transition(&mut self, before: Phase, after: Phase) -> Result<(), String> {
        if self.phase != before {
            self.retain();
            return Err("fixture 阶段不允许重放".into());
        }
        self.phase = after;
        Ok(())
    }

    pub fn retain(&mut self) {
        self.phase = Phase::Retained;
    }

    pub fn validate_cleanup(&self, observed: &Observation) -> Result<(), String> {
        if !matches!(self.phase, Phase::TestPassed | Phase::DropIntent)
            || self.plan.identity != observed.identity
            || observed.lock_owner != Some(self.plan.identity.connection_id)
            || !observed.plan_identity_matches
            || !observed.plan_content_matches
            || !observed.journal_matches
            || !observed.business_closed
            || !observed.schema_exists
            || !observed.process_visibility
            || observed.active_connections != 0
        {
            return Err("fixture 清理归属未获完整确认，禁止 DROP".into());
        }
        Ok(())
    }

    pub fn drop_intent(&mut self, observed: &Observation) -> Result<(), String> {
        if let Err(error) = self.validate_cleanup(observed) {
            self.retain();
            return Err(error);
        }
        self.transition(Phase::TestPassed, Phase::DropIntent)
    }

    pub fn dropped(&mut self, absent: bool) -> Result<(), String> {
        if !absent {
            self.retain();
            return Err("DROP 后 schema 状态不明确，禁止重放".into());
        }
        self.transition(Phase::DropIntent, Phase::Dropped)
    }
}

struct OwnedFile {
    path: PathBuf,
    file: File,
    identity: Handle,
    expected: Vec<u8>,
}

impl OwnedFile {
    fn create(path: PathBuf) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| "fixture 证据必须由本次排他创建，已有文件不得覆盖")?;
        file.try_lock()
            .map_err(|_| "无法独占锁定 fixture 证据文件")?;
        let identity = Handle::from_file(file.try_clone().map_err(|_| "无法持有证据文件")?)
            .map_err(|_| "无法确认 fixture 证据文件身份")?;
        Ok(Self {
            path,
            file,
            identity,
            expected: Vec::new(),
        })
    }

    fn verify(&mut self) -> Result<(), String> {
        let metadata = fs::symlink_metadata(&self.path).map_err(|_| "fixture 证据路径不可确认")?;
        if !super::ordinary(&metadata)
            || !metadata.is_file()
            || metadata.len() != self.expected.len() as u64
            || self.identity != Handle::from_path(&self.path).map_err(|_| "证据文件身份不可确认")?
        {
            return Err("fixture 证据文件身份或长度变化".into());
        }
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| "无法校验证据完整内容")?;
        let mut bytes = Vec::new();
        (&self.file)
            .take(self.expected.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "无法校验证据完整内容")?;
        if bytes != self.expected {
            return Err("fixture 证据完整内容变化".into());
        }
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.verify()?;
        self.file
            .write_all(bytes)
            .and_then(|_| self.file.sync_all())
            .map_err(|_| "fixture 证据落盘结果不明确，保留现场")?;
        self.expected.extend_from_slice(bytes);
        self.verify()
    }
}

pub(super) struct Evidence {
    ancestors: Vec<(PathBuf, Handle)>,
    plan: OwnedFile,
    journal: OwnedFile,
    metadata: Plan,
}

fn verify_ancestors(ancestors: &[(PathBuf, Handle)]) -> Result<(), String> {
    for (path, identity) in ancestors {
        let metadata = fs::symlink_metadata(path).map_err(|_| "fixture 目录不可确认")?;
        if !super::ordinary(&metadata)
            || !metadata.is_dir()
            || *identity != Handle::from_path(path).map_err(|_| "fixture 目录身份不可确认")?
        {
            return Err("fixture 证据目录身份变化".into());
        }
    }
    Ok(())
}

impl Evidence {
    pub fn plan_path(&self) -> &Path {
        &self.plan.path
    }

    pub fn create(root: &Path, plan: &Plan) -> Result<Self, String> {
        let directory = root.join(".local-tests/mysql/fixtures");
        let mut ancestors = Vec::new();
        for path in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
            verify_ancestors(&ancestors)?;
            match fs::symlink_metadata(path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    verify_ancestors(&ancestors)?;
                    fs::create_dir(path).map_err(|_| "无法创建 fixture 证据目录")?;
                }
                Err(_) => return Err("无法检查 fixture 证据目录".into()),
            }
            let metadata = fs::symlink_metadata(path).map_err(|_| "无法检查证据目录")?;
            if !super::ordinary(&metadata) || !metadata.is_dir() {
                return Err("fixture 证据祖先不能是链接或重解析点".into());
            }
            ancestors.push((
                path.to_path_buf(),
                Handle::from_path(path).map_err(|_| "无法持有 fixture 目录身份")?,
            ));
            verify_ancestors(&ancestors)?;
        }
        verify_ancestors(&ancestors)?;
        let plan_file = OwnedFile::create(directory.join(format!("{}.plan.json", plan.schema)))?;
        verify_ancestors(&ancestors)?;
        let journal = OwnedFile::create(directory.join(format!("{}.journal.jsonl", plan.schema)))?;
        verify_ancestors(&ancestors)?;
        let mut value = Self {
            ancestors,
            plan: plan_file,
            journal,
            metadata: plan.clone(),
        };
        value.verify()?;
        value.plan.append(&plan.bytes()?)?;
        value.record("planned")?;
        Ok(value)
    }

    pub fn verify(&mut self) -> Result<(), String> {
        verify_ancestors(&self.ancestors)?;
        self.plan.verify()?;
        self.journal.verify()
    }

    pub fn record(&mut self, stage: &'static str) -> Result<(), String> {
        self.verify()?;
        let plan = &self.metadata;
        let mut bytes = serde_json::to_vec(&json!({"schema": plan.schema,
            "scope": plan.scope, "test": plan.test, "server_uuid": plan.identity.server_uuid,
            "connection_id": plan.identity.connection_id,
            "lock_key": plan.schema, "lock_owner": plan.identity.connection_id, "stage": stage}))
        .map_err(|_| "无法编码 fixture 阶段")?;
        bytes.push(b'\n');
        self.journal.append(&bytes)
    }

    pub fn failure(&mut self, stage: &'static str) {
        if self.record(stage).is_err() {
            eprintln!("fixture failure 诊断无法追加；原计划与 journal 保留，不覆盖或重试写入");
        }
    }

    pub fn observed(&mut self, stage: &'static str, observed: &Observation) -> Result<(), String> {
        self.verify()?;
        let plan = &self.metadata;
        let mut bytes = serde_json::to_vec(&json!({"schema": plan.schema,
            "scope": plan.scope, "test": plan.test,
            "server_uuid": observed.identity.server_uuid,
            "connection_id": observed.identity.connection_id,
            "lock_key": plan.schema, "lock_owner": observed.lock_owner,
            "process_visibility": observed.process_visibility,
            "active_connections": observed.active_connections,
            "business_closed": observed.business_closed,
            "schema_exists": observed.schema_exists, "stage": stage}))
        .map_err(|_| "无法编码 fixture 清理观察")?;
        bytes.push(b'\n');
        self.journal.append(&bytes)
    }
}

pub(super) struct AdminSession {
    pool: DatabaseConnection,
    session: PoolConnection<MySql>,
    lock_key: Option<String>,
}

impl AdminSession {
    pub async fn acquire(pool: DatabaseConnection) -> Result<Self, String> {
        let session = pool.get_mysql_connection_pool().acquire().await;
        match session {
            Ok(mut session) => {
                session.close_on_drop();
                Ok(Self {
                    pool,
                    session,
                    lock_key: None,
                })
            }
            Err(_) => {
                let _ = pool.close().await;
                Err("无法持有 MySQL 管理连接".into())
            }
        }
    }

    pub async fn identity(&mut self) -> Result<Identity, String> {
        let (server_uuid, connection_id) = sqlx::query_as::<_, (String, u64)>(sqlx::AssertSqlSafe(
            "SELECT @@server_uuid, CONNECTION_ID()",
        ))
        .fetch_one(&mut *self.session)
        .await
        .map_err(|_| "无法确认管理连接身份")?;
        Ok(Identity {
            server_uuid,
            connection_id,
        })
    }

    pub async fn lock(&mut self, schema: &str) -> Result<(), String> {
        validate_schema(schema)?;
        let acquired = sqlx::query_scalar::<_, Option<i64>>(sqlx::AssertSqlSafe(
            "SELECT CAST(GET_LOCK(?, 0) AS SIGNED)",
        ))
        .bind(schema)
        .fetch_one(&mut *self.session)
        .await
        .map_err(|_| "fixture 会话锁获取结果不明确，停止")?;
        if acquired != Some(1) {
            return Err("未获得 fixture 会话锁，不强抢".into());
        }
        self.lock_key = Some(schema.to_owned());
        Ok(())
    }

    pub async fn exists(&mut self, schema: &str) -> Result<bool, String> {
        let count = sqlx::query_scalar::<_, u64>(sqlx::AssertSqlSafe(
            "SELECT CAST(COUNT(*) AS UNSIGNED) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
        ))
        .bind(schema)
        .fetch_one(&mut *self.session)
        .await
        .map_err(|error| match error {
            sqlx::Error::ColumnDecode { index, source } => {
                format!("schema 存在性计数解码失败（列 {index}）：{source}；只读检查未通过，未执行建库")
            }
            sqlx::Error::Database(error) => format!(
                "schema 存在性查询失败（数据库错误码 {}）；检查查询权限，未执行建库",
                error.code().as_deref().unwrap_or("未提供")
            ),
            _ => "schema 存在性查询失败；检查连接与驱动状态，未执行建库".into(),
        })?;
        Ok(count != 0)
    }

    pub async fn ddl(&mut self, schema: &str, create: bool) -> Result<(), String> {
        validate_schema(schema)?;
        let sql = if create {
            format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4")
        } else {
            format!("DROP DATABASE `{schema}`")
        };
        sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(&mut *self.session)
            .await
            .map(|_| ())
            .map_err(|_| "fixture DDL 响应不明确，禁止重放".into())
    }

    pub async fn observe(
        &mut self,
        schema: &str,
        files_match: bool,
    ) -> Result<Observation, String> {
        let identity = self.identity().await?;
        let lock_owner = sqlx::query_scalar::<_, Option<u64>>(sqlx::AssertSqlSafe(
            "SELECT CAST(IS_USED_LOCK(?) AS UNSIGNED)",
        ))
        .bind(schema)
        .fetch_one(&mut *self.session)
        .await
        .map_err(|_| "无法确认会话锁持有者")?;
        // 无 PROCESS 权限时 PROCESSLIST 只展示部分连接，不能据此授权删除。
        let grants = sqlx::raw_sql(sqlx::AssertSqlSafe("SHOW GRANTS FOR CURRENT_USER()"))
            .fetch_all(&mut *self.session)
            .await
            .map_err(|_| "无法确认进程列表全局可见性")?;
        let mut visible = false;
        for row in grants {
            let grant: String = row.try_get(0).map_err(|_| "无法读取全局权限证明")?;
            visible |= permits_processlist(&grant);
        }
        let active_connections = sqlx::query_scalar::<_, u64>(sqlx::AssertSqlSafe(
            "SELECT CAST(COUNT(*) AS UNSIGNED) FROM information_schema.PROCESSLIST WHERE DB = ? AND ID <> CONNECTION_ID()"))
            .bind(schema).fetch_one(&mut *self.session).await.map_err(|_| "无法确认本 schema 活动连接")?;
        let schema_exists = self.exists(schema).await?;
        Ok(Observation {
            identity,
            lock_owner,
            plan_identity_matches: files_match,
            plan_content_matches: files_match,
            journal_matches: files_match,
            business_closed: true,
            schema_exists,
            process_visibility: visible,
            active_connections,
        })
    }

    pub async fn release(self) -> Result<(), String> {
        let Self {
            pool,
            mut session,
            lock_key,
        } = self;
        let released = if let Some(key) = lock_key {
            sqlx::query_scalar::<_, Option<i64>>(sqlx::AssertSqlSafe("SELECT RELEASE_LOCK(?)"))
                .bind(key)
                .fetch_one(&mut *session)
                .await
                .map_err(|_| "会话锁释放结果不明确".to_owned())
                .and_then(|value| {
                    if value == Some(1) {
                        Ok(())
                    } else {
                        Err("会话锁释放未确认".into())
                    }
                })
        } else {
            Ok(())
        };
        let closed = session
            .close()
            .await
            .map_err(|_| "管理连接关闭未确认".to_owned());
        let pool_closed = pool
            .close()
            .await
            .map_err(|_| "管理池关闭未确认".to_owned());
        released.and(closed).and(pool_closed)
    }
}

#[cfg(test)]
#[path = "mysql_fixture_ownership_tests.rs"]
mod tests;
