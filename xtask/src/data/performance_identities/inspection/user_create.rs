//! 使用同一只读查询核对用户创建或改密后的数据库像。

use std::path::Path;

use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339, macros::format_description};

use super::{super::state, array, bindings, open, ownership, plan, runtime_proof, text};
use crate::{
    Result, devex::job_evidence::protocol::Session, recovery::clone::native::json as document,
};

const NICKNAME: &str = "运行时性能普通用户";

pub(in crate::data::performance_identities) fn observe_created_user(
    plan: &Value,
    group: &Value,
    user: &Value,
    role_id: &str,
    expected_user_id: Option<&str>,
    audit: Option<&Value>,
) -> Result<Value> {
    if expected_user_id.is_none() && audit.is_none() {
        return Err("核对新用户需要原 API 用户标识或已绑定的创建请求".into());
    }
    let audit = audit.map(AuditWindow::parse).transpose()?;
    observe(
        plan,
        group,
        user,
        role_id,
        expected_user_id,
        true,
        audit.as_ref(),
    )
}

pub(in crate::data::performance_identities) fn observe_activated_user(
    plan: &Value,
    group: &Value,
    user: &Value,
    role_id: &str,
    expected_user_id: &str,
) -> Result<()> {
    observe(
        plan,
        group,
        user,
        role_id,
        Some(expected_user_id),
        false,
        None,
    )
    .map(|_| ())
}

struct Subject<'a> {
    tenant: &'a str,
    username: &'a str,
    role_code: &'a str,
    operator: &'a str,
}

impl<'a> Subject<'a> {
    fn from_plan(plan: &Value, group: &'a Value, user: &'a Value) -> Result<Self> {
        if !array(&plan["groups"])?.contains(group) || !array(&group["users"])?.contains(user) {
            return Err("待核对用户不属于原身份计划".into());
        }
        Ok(Self {
            tenant: text(group, "tenant_id")?,
            username: text(user, "username")?,
            role_code: text(group, "role_code")?,
            operator: text(&group["admin"], "username")?,
        })
    }
}

fn observe(
    source: &Value,
    group: &Value,
    user: &Value,
    role_id: &str,
    expected_user_id: Option<&str>,
    must_change_password: bool,
    audit: Option<&AuditWindow<'_>>,
) -> Result<Value> {
    state::id(&json!(role_id))?;
    if let Some(id) = expected_user_id {
        state::id(&json!(id))?;
    }
    let backend = Path::new(text(&source["environment"], "backend_dir")?);
    let private = plan::load(source, backend)?;
    let subject = Subject::from_plan(source, group, user)?;
    let proof = runtime_proof(&private)?;
    let directory = Path::new(text(&proof["runtime"], "api_config_dir")?);
    let table = crate::rate_limit::load_app_table(backend, Some(directory), "test")
        .map_err(|_| "用户核对无法读取已绑定的 API 配置")?;
    bindings(&private, &serde_json::to_value(table)?)?;

    let database = &private["database"];
    let connection = &database["control"];
    let schema = text(connection, "database")?;
    let statement = user_query(schema, &subject, role_id, must_change_password, audit);
    let mut session = open(database, connection)?;
    let result = (|| {
        ownership(&mut session, connection, &private, "control")?;
        let rows = session
            .query(&statement)
            .map_err(|_| "用户数据库像的只读查询失败")?;
        verified_image(&rows, role_id, expected_user_id, audit.is_some())
    })();
    let closed = session.close().map_err(|_| "用户核对的只读会话关闭失败");
    let image = result?;
    closed?;
    if private != plan::load(source, backend)? {
        return Err("用户核对期间身份环境来源变化".into());
    }
    Ok(image)
}

fn user_query(
    schema: &str,
    subject: &Subject<'_>,
    role_id: &str,
    must_change_password: bool,
    audit: Option<&AuditWindow<'_>>,
) -> String {
    let tenant = literal(subject.tenant);
    let username = literal(subject.username);
    let nickname = literal(NICKNAME);
    let role_code = literal(subject.role_code);
    let password_change_required = u8::from(must_change_password);
    let create_defaults = if must_change_password {
        "AND u.authorization_version=1 AND u.login_ip IS NULL AND u.login_date IS NULL
         AND u.created_at=u.updated_at"
    } else {
        ""
    };
    let audit_projection = audit.map_or_else(
        || "NULL".to_owned(),
        |window| window.projection(schema, &tenant, subject.operator),
    );
    let within_window = audit.map_or_else(
        || "1".to_owned(),
        |window| format!("u.created_at BETWEEN {} AND {}", window.start, window.end),
    );
    format!(
        "SELECT JSON_OBJECT(
            'id',CAST(u.id AS CHAR),
            'created_at',DATE_FORMAT(u.created_at,'%Y-%m-%dT%H:%i:%s.%fZ'),
            'shape',IF(CAST(u.tenant_id AS BINARY)=CAST({tenant} AS BINARY)
              AND CAST(u.username AS BINARY)=CAST({username} AS BINARY)
              AND CAST(u.nickname AS BINARY)=CAST({nickname} AS BINARY)
              AND u.status='1' AND u.del_flag='0'
              AND u.password_change_required={password_change_required}
              AND u.authorization_version>=1
              AND OCTET_LENGTH(u.email)=0 AND OCTET_LENGTH(u.phone)=0
              AND u.dept_id IS NULL AND u.avatar IS NULL AND u.avatar_file_id IS NULL
              AND u.preferred_locale IS NULL AND u.remark IS NULL
              AND ({within_window}) {create_defaults},1,0),
            'role_edges',(SELECT COUNT(*) FROM `{schema}`.sys_user_role e WHERE e.user_id=u.id),
            'expected_role_edges',(SELECT COUNT(*) FROM `{schema}`.sys_user_role e
               JOIN `{schema}`.sys_role r ON r.id=e.role_id
               WHERE e.user_id=u.id AND e.role_id={role_id}
                 AND CAST(e.tenant_id AS BINARY)=CAST({tenant} AS BINARY)
                 AND CAST(r.tenant_id AS BINARY)=CAST({tenant} AS BINARY)
                 AND CAST(r.code AS BINARY)=CAST({role_code} AS BINARY)
                 AND r.status='1' AND r.del_flag='0' AND r.is_super=0 AND r.data_scope='1'),
            'audit',{audit_projection})
         FROM `{schema}`.sys_user u
         WHERE CAST(u.tenant_id AS BINARY)=CAST({tenant} AS BINARY)
           AND CAST(u.username AS BINARY)=CAST({username} AS BINARY)
         LIMIT 2"
    )
}

struct AuditWindow<'a> {
    request_id: &'a str,
    start: String,
    end: String,
}

impl<'a> AuditWindow<'a> {
    fn parse(value: &'a Value) -> Result<Self> {
        document::exact(
            value,
            &["request_id", "window_start", "window_end"],
            "原用户创建请求",
        )?;
        let request_id = text(value, "request_id")?;
        if request_id.is_empty()
            || request_id.len() > 36
            || !request_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("原用户创建请求标识无效".into());
        }
        let start = sql_micros(text(value, "window_start")?, true)?;
        let end = sql_micros(text(value, "window_end")?, false)?;
        if start > end {
            return Err("原用户创建时间窗缺少有效的微秒时刻".into());
        }
        Ok(Self {
            request_id,
            start: datetime_literal(start)?,
            end: datetime_literal(end)?,
        })
    }

    fn projection(&self, schema: &str, tenant: &str, administrator: &str) -> String {
        let request = literal(self.request_id);
        let administrator = literal(administrator);
        let start = &self.start;
        let end = &self.end;
        format!(
            "CAST((SELECT JSON_OBJECT(
                'request_matches',COUNT(CASE WHEN x.request_match THEN 1 END),
                'route_matches',COUNT(CASE WHEN x.route_match THEN 1 END),
                'matches',COUNT(CASE WHEN x.request_match AND x.route_match AND x.shape THEN 1 END),
                'id',MIN(CASE WHEN x.request_match AND x.route_match AND x.shape
                           THEN CAST(x.id AS CHAR) END),
                'created_at',MIN(CASE WHEN x.request_match AND x.route_match AND x.shape
                           THEN DATE_FORMAT(x.created_at,'%Y-%m-%dT%H:%i:%s.%fZ') END))
             FROM (SELECT e.id,e.created_at,
                CAST(e.payload->>'$.request_id' AS BINARY)=CAST({request} AS BINARY)
                    AS request_match,
                (CAST(e.payload->>'$.command.request_method' AS BINARY)='POST'
                 AND CAST(e.payload->>'$.command.oper_url' AS BINARY)='/api/v1/system/users'
                 AND e.created_at BETWEEN {start} AND {end}) AS route_match,
                (CAST(e.payload->>'$.tenant_id' AS BINARY)=CAST({tenant} AS BINARY)
                 AND CAST(e.payload->>'$.command.oper_name' AS BINARY)=CAST({administrator} AS BINARY)
                 AND CAST(e.payload->>'$.command.status' AS BINARY)='success'
                 AND e.aggregate_type='operation_audit'
                 AND CAST(e.payload->>'$.event_id' AS BINARY)=CAST(e.aggregate_id AS BINARY)
                 AND CAST(e.dedupe_key AS BINARY)=CAST(e.aggregate_id AS BINARY)) AS shape
              FROM `{schema}`.sys_outbox_event e
              WHERE CAST(e.tenant_id AS BINARY)=CAST({tenant} AS BINARY)
                AND e.event_type='audit.operation') x) AS JSON)"
        )
    }
}

fn sql_micros(value: &str, round_up: bool) -> Result<i64> {
    if value.as_bytes().get(17..19) == Some(b"60") {
        return Err("原用户创建时间不能包含闰秒".into());
    }
    let nanos = OffsetDateTime::parse(value, &Rfc3339)?.unix_timestamp_nanos();
    Ok(i64::try_from(nanos.div_euclid(1000))? + i64::from(round_up && nanos.rem_euclid(1000) != 0))
}

fn datetime_literal(micros: i64) -> Result<String> {
    let value = OffsetDateTime::from_unix_timestamp_nanos(i128::from(micros) * 1000)?;
    if !(1000..=9999).contains(&value.year()) {
        return Err("原用户创建时间超出数据库范围".into());
    }
    Ok(format!(
        "CAST({} AS DATETIME(6))",
        literal(&value.format(format_description!(
            "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:6]"
        ))?)
    ))
}

fn literal(value: &str) -> String {
    format!(
        "CONVERT(X'{}' USING utf8mb4)",
        hex::encode(value.as_bytes())
    )
}

fn verified_image(
    rows: &[Value],
    role_id: &str,
    expected_user_id: Option<&str>,
    has_audit: bool,
) -> Result<Value> {
    let [row] = rows else {
        return Err("原计划用户缺失或同名记录不唯一".into());
    };
    let id = state::id(&row["id"])?;
    if expected_user_id.is_some_and(|expected| expected != id)
        || row["shape"].as_u64() != Some(1)
        || row["role_edges"].as_u64() != Some(1)
        || row["expected_role_edges"].as_u64() != Some(1)
    {
        return Err("原计划用户的身份、创建字段或角色后像不符".into());
    }
    let audit = if has_audit {
        let proof = &row["audit"];
        if ["request_matches", "route_matches", "matches"]
            .iter()
            .any(|key| proof[*key].as_u64() != Some(1))
        {
            return Err("原用户创建审计缺失、重复或身份与时间窗不符".into());
        }
        json!({"id":state::id(&proof["id"])?,"created_at":created_time(proof)?})
    } else {
        Value::Null
    };
    Ok(json!({"id":id,"role_id":role_id,"created_at":created_time(row)?,"audit":audit}))
}

fn created_time(value: &Value) -> Result<&str> {
    let created = text(value, "created_at")?;
    sql_micros(created, false)?;
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_window_compares_microsecond_instants_without_widening_the_request() {
        let start = "2026-11-25T23:39:54.000000001+08:00";
        let end = "2026-11-25T15:39:54.000001999Z";
        assert_eq!(
            sql_micros(start, true).unwrap(),
            sql_micros(end, false).unwrap()
        );
        let accepted = json!({"request_id":"request-123","window_start":start,"window_end":end});
        assert!(AuditWindow::parse(&accepted).is_ok());
        let no_instant = json!({"request_id":"request-123","window_start":start,"window_end":"2026-11-25T15:39:54.000000999Z"});
        assert!(AuditWindow::parse(&no_instant).is_err());
        assert!(sql_micros("2016-12-31T23:59:60Z", false).is_err());
    }

    #[test]
    fn verified_create_image_requires_exact_identity_edge_and_request_counts() {
        let row = json!({"id":"21","created_at":"2026-11-25T15:39:54.000001Z","shape":1,
            "role_edges":1,"expected_role_edges":1,"audit":{"request_matches":1,"route_matches":1,
            "matches":1,"id":"31","created_at":"2026-11-25T15:39:54.000002Z"}});
        let result = verified_image(std::slice::from_ref(&row), "41", None, true).unwrap();
        assert_eq!(result["id"], "21");
        assert_eq!(result["audit"]["id"], "31");
        assert!(verified_image(&[row.clone(), row.clone()], "41", None, true).is_err());
        assert!(verified_image(std::slice::from_ref(&row), "41", Some("22"), false).is_err());
        for path in [
            "/shape",
            "/role_edges",
            "/expected_role_edges",
            "/audit/request_matches",
            "/audit/route_matches",
            "/audit/matches",
        ] {
            let mut changed = row.clone();
            *changed.pointer_mut(path).unwrap() = json!(2);
            assert!(
                verified_image(&[changed], "41", None, true).is_err(),
                "{path}"
            );
        }
    }
}
