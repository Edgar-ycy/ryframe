use std::{
    collections::BTreeMap,
    io::{BufRead, Read},
    time::Duration,
};

use regex::Regex;
use serde_json::{Value, json};

use super::{model, mysql::MysqlSession, sql};
use crate::recovery::clone::native::json::parse_fragment;

pub(crate) const INPUT_LIMIT: usize = 32_768;
pub(crate) type EvidenceResult<T> = Result<T, &'static str>;

/// 私有协议不接受任意 SQL；调用方只提供固定业务关联与登记连接。
pub(crate) trait Session {
    fn query(&mut self, statement: &str) -> EvidenceResult<Vec<Value>>;
    fn close(&mut self) -> EvidenceResult<()>;
}

impl Session for MysqlSession {
    fn query(&mut self, statement: &str) -> EvidenceResult<Vec<Value>> {
        self.execute(statement, Duration::from_secs(10))
    }

    fn close(&mut self) -> EvidenceResult<()> {
        MysqlSession::close(self)
    }
}

pub(crate) fn exact(value: &Value, fields: &[&str], reason: &'static str) -> EvidenceResult<()> {
    let object = value.as_object().ok_or(reason)?;
    if object.len() != fields.len() || fields.iter().any(|key| !object.contains_key(*key)) {
        return Err(reason);
    }
    Ok(())
}

pub(crate) fn parse(bytes: &[u8], reason: &'static str) -> EvidenceResult<Value> {
    if bytes.len() > INPUT_LIMIT {
        return Err(reason);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| reason)?;
    parse_fragment(text, "任务证据协议").map_err(|_| reason)
}

pub(crate) fn read_line(reader: &mut impl BufRead) -> EvidenceResult<Option<Value>> {
    let mut line = Vec::new();
    let length = reader
        .take((INPUT_LIMIT + 1) as u64)
        .read_until(b'\n', &mut line)
        .map_err(|_| "wait_input_limit")?;
    if length == 0 {
        return Ok(None);
    }
    if length > INPUT_LIMIT || line.last() != Some(&b'\n') {
        return Err("wait_input_limit");
    }
    parse(&line, "invalid_wait_request").map(Some)
}

pub(crate) fn binding(
    value: &Value,
    scope: &str,
    environment: &BTreeMap<String, String>,
) -> EvidenceResult<()> {
    exact(
        value,
        &["control", "server_uuid", "mysql_client"],
        "invalid_database_binding",
    )?;
    if !matches(scope, r"^[a-z0-9][a-z0-9-]{2,63}$") {
        return Err("invalid_scope");
    }
    let control = &value["control"];
    exact(
        control,
        &[
            "host",
            "port",
            "database",
            "username",
            "password_env",
            "tls_mode",
        ],
        "invalid_control_binding",
    )?;
    let password = control["password_env"]
        .as_str()
        .ok_or("invalid_control_binding")?;
    if !matches(
        control["database"].as_str().unwrap_or_default(),
        r"^[a-zA-Z0-9_]{1,64}$",
    ) || !matches!(control["host"].as_str(), Some("127.0.0.1" | "localhost"))
        || !control["port"]
            .as_u64()
            .is_some_and(|port| (1..=65535).contains(&port))
        || !matches!(
            control["tls_mode"].as_str(),
            Some("required" | "preferred" | "disabled")
        )
        || control["username"].as_str().is_none_or(str::is_empty)
        || !matches(password, r"^[A-Z][A-Z0-9_]*$")
    {
        return Err("invalid_control_binding");
    }
    if !matches(
        value["server_uuid"].as_str().unwrap_or_default(),
        r"^[a-f0-9-]{36}$",
    ) {
        return Err("invalid_server_identity");
    }
    #[cfg(windows)]
    let present = match environment
        .keys()
        .filter(|name| name.eq_ignore_ascii_case(password))
        .count()
    {
        0 => false,
        1 => true,
        _ => return Err("ambiguous_database_password_environment"),
    };
    #[cfg(not(windows))]
    let present = environment.contains_key(password);
    if !present {
        return Err("missing_database_password_environment");
    }
    Ok(())
}

pub(crate) fn owner(
    session: &mut impl Session,
    binding: &Value,
    scope: &str,
) -> EvidenceResult<()> {
    let database = binding["control"]["database"]
        .as_str()
        .ok_or("invalid_control_binding")?;
    let statement = format!(
        "SELECT JSON_OBJECT('scope',scope_id,'marker',marker,'server',@@server_uuid) FROM `{database}`.ryframe_resource_ownership WHERE resource_kind='control'"
    );
    let rows = session
        .query(&statement)
        .map_err(|_| "wrong_database_ownership")?;
    if rows.len() != 1
        || rows[0]["scope"] != scope
        || rows[0]["marker"] != format!("ryframe-owner:v1:{scope}:control")
    {
        return Err("wrong_database_ownership");
    }
    if rows[0]["server"] != binding["server_uuid"] {
        return Err("wrong_server_identity");
    }
    Ok(())
}

pub(crate) fn collect(
    session: &mut impl Session,
    binding: &Value,
    scope: &str,
    selection: &Value,
) -> EvidenceResult<Value> {
    model::selection(selection)?;
    session.query("SET SESSION time_zone='+00:00'")?;
    session.query("START TRANSACTION READ ONLY")?;
    owner(session, binding, scope)?;
    let database = binding["control"]["database"]
        .as_str()
        .ok_or("invalid_control_binding")?;
    let rows = session.query(&sql::job_sql(database, selection)?)?;
    let row = model::associated_job(&rows, selection)?;
    let id = row["job_id"].as_str().ok_or("invalid_job_identifier")?;
    let attempts = session.query(&sql::attempts_sql(database, id)?)?;
    if attempts.len() > 10_000 {
        return Err("attempt_history_limit");
    }
    let mut result = model::evidence(&row, &attempts);
    let object = result.as_object_mut().ok_or("database_collection_failed")?;
    object.insert("scope_id".into(), json!(scope));
    object.insert("kind".into(), selection["kind"].clone());
    object.insert("business_id".into(), selection["id"].clone());
    object.insert("format_version".into(), json!(1));
    Ok(result)
}

fn matches(value: &str, pattern: &str) -> bool {
    Regex::new(pattern).is_ok_and(|regex| regex.is_match(value))
}
