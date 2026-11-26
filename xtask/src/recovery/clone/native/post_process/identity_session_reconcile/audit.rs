//! 只读核对强退的成功审计，持久证据仅使用 UUID 和脱敏摘要。

use super::super::read_target_control;
use crate::{
    Result,
    recovery::clone::native::{
        export::transport::SystemTransport, target_verify::storage::StorageInputs,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::{Uuid, Variant};

const ROUTE: &str = "/api/v1/system/online/{sid}";

pub(super) fn confirm(
    target: &StorageInputs<'_>,
    actor: &str,
    sid: &str,
    request_id: Option<&str>,
    started_at: &str,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<Value> {
    let audit = Audit::new(actor, sid, request_id, started_at)?;
    let sql = audit_query(audit.request_id);
    read_target_control(
        target,
        ("identity-session-force-logout-audit", &sql),
        &SystemTransport,
        checkpoint,
        |raw| audit.decode(raw),
    )
}

struct Audit<'a> {
    request_id: &'a str,
    request_millis: i64,
    started_at: OffsetDateTime,
    actor_sha256: String,
    concrete_sha256: String,
    template_sha256: String,
}

impl<'a> Audit<'a> {
    fn new(actor: &str, sid: &str, request_id: Option<&'a str>, started_at: &str) -> Result<Self> {
        let request_id = request_id.ok_or("缺少受管强退响应的服务端 UUIDv7，审计不能确认")?;
        let request_millis = uuid7_millis(request_id)?;
        let started_at = utc(started_at)?;
        if i128::from(request_millis) < started_at.unix_timestamp_nanos().div_euclid(1_000_000)
            || actor.is_empty()
            || actor.chars().any(char::is_control)
            || !sid
                .strip_prefix("s-")
                .is_some_and(|id| canonical_uuid(id, 4).is_some())
        {
            return Err("强退审计无法绑定原时间、执行人或会话身份".into());
        }
        Ok(Self {
            request_id,
            request_millis,
            started_at,
            actor_sha256: sha256(actor),
            concrete_sha256: sha256(&ROUTE.replace("{sid}", sid)),
            template_sha256: sha256(ROUTE),
        })
    }

    fn decode(&self, raw: &str) -> Result<Value> {
        let mut outbox = None;
        let mut projection = None;
        for line in raw.lines() {
            let values = line.split('\t').collect::<Vec<_>>();
            let &[
                origin,
                event_id,
                request_id,
                observed_at,
                path_sha256,
                actor_sha256,
                "1",
            ] = values.as_slice()
            else {
                return Err("使用同一服务端请求的审计行缺失或未满足成功绑定".into());
            };
            let row =
                self.match_row(event_id, request_id, observed_at, path_sha256, actor_sha256)?;
            let previous = match origin {
                "outbox" => outbox.replace(row),
                "projection" => projection.replace(row),
                _ => return Err("强退审计来自未登记的证据表".into()),
            };
            if previous.is_some() {
                return Err("同一请求在一张审计表中存在多个候选，保留现场".into());
            }
        }
        let Some(event) = outbox.as_ref().or(projection.as_ref()) else {
            return Err("没有发现绑定此服务端请求的成功强退审计".into());
        };
        if let (Some(outbox), Some(projection)) = (&outbox, &projection)
            && (outbox.event_id != projection.event_id
                || outbox.path_sha256 != projection.path_sha256
                || projection.observed_at < outbox.observed_at)
        {
            return Err("Outbox 与审计投影没有指向同一强退事件，保留现场".into());
        }
        Ok(json!({
            "format_version":1,"kind":"devex-identity-session-force-logout-audit",
            "tenant_id":"system","request_method":"DELETE","event_id":event.event_id,
            "request_id":self.request_id,"actor_sha256":self.actor_sha256,
            "path_sha256":event.path_sha256,"path_kind":event.path_kind,
            "started_at":self.started_at.format(&Rfc3339)?,
            "outbox_observed_at":observed_at(outbox.as_ref())?,
            "projection_observed_at":observed_at(projection.as_ref())?,
        }))
    }

    fn match_row<'b>(
        &self,
        event_id: &'b str,
        request_id: &str,
        observed_at: &str,
        path_sha256: &'b str,
        actor_sha256: &str,
    ) -> Result<AuditRow<'b>> {
        let event_millis = uuid7_millis(event_id)?;
        let observed_at = utc(observed_at)?;
        let path_kind = if path_sha256 == self.concrete_sha256 {
            "concrete"
        } else if path_sha256 == self.template_sha256 {
            "route-template"
        } else {
            return Err("成功审计的路径摘要不同于受管强退".into());
        };
        if request_id != self.request_id
            || event_id == self.request_id
            || event_millis < self.request_millis
            || observed_at.unix_timestamp() < event_millis.div_euclid(1000)
            || actor_sha256 != self.actor_sha256
        {
            return Err("成功审计与原强退请求、执行人或起点不匹配".into());
        }
        Ok(AuditRow {
            event_id,
            observed_at,
            path_sha256,
            path_kind,
        })
    }
}

struct AuditRow<'a> {
    event_id: &'a str,
    observed_at: OffsetDateTime,
    path_sha256: &'a str,
    path_kind: &'static str,
}

fn observed_at(row: Option<&AuditRow<'_>>) -> Result<Option<String>> {
    Ok(row
        .map(|row| row.observed_at.format(&Rfc3339))
        .transpose()?)
}

fn utc(value: &str) -> Result<OffsetDateTime> {
    let value =
        OffsetDateTime::parse(value, &Rfc3339).map_err(|_| "强退审计时间不是 UTC RFC3339")?;
    if !value.offset().is_utc() || value.unix_timestamp() <= 0 {
        return Err("强退审计时间不在有效 UTC 范围".into());
    }
    Ok(value)
}

fn canonical_uuid(text: &str, version: usize) -> Option<Uuid> {
    Uuid::parse_str(text).ok().filter(|uuid| {
        uuid.get_version_num() == version
            && uuid.get_variant() == Variant::RFC4122
            && uuid.hyphenated().to_string() == text
    })
}

fn uuid7_millis(text: &str) -> Result<i64> {
    let uuid = canonical_uuid(text, 7).ok_or("强退审计缺少规范服务端 UUIDv7")?;
    let [a, b, c, d, e, f, ..] = *uuid.as_bytes();
    Ok(i64::from_be_bytes([0, 0, a, b, c, d, e, f]))
}

fn sha256(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

// 同一请求先找审计事件，再读取相同 event_id 的两种持久表示。只返回受限的标识、摘要和校验位。
fn audit_query(request_id: &str) -> String {
    let uuid_pattern = r"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";
    format!(
        r"WITH outbox_records AS (
            SELECT tenant_id, aggregate_type, aggregate_id, dedupe_key, created_at, status AS delivery,
                JSON_UNQUOTE(JSON_EXTRACT(payload,'$.event_id')) AS event_id,
                JSON_UNQUOTE(JSON_EXTRACT(payload,'$.request_id')) AS request_id,
                JSON_UNQUOTE(JSON_EXTRACT(payload,'$.tenant_id')) AS payload_tenant,
                JSON_UNQUOTE(JSON_EXTRACT(payload,'$.command.status')) AS operation_status,
                JSON_UNQUOTE(JSON_EXTRACT(payload,'$.command.request_method')) AS request_method,
                JSON_UNQUOTE(JSON_EXTRACT(payload,'$.command.method')) AS operation_method,
                JSON_UNQUOTE(JSON_EXTRACT(payload,'$.command.business_type')) AS business_type,
                JSON_UNQUOTE(JSON_EXTRACT(payload,'$.command.oper_url')) AS oper_url,
                JSON_UNQUOTE(JSON_EXTRACT(payload,'$.command.oper_name')) AS oper_name
            FROM sys_outbox_event
            WHERE CAST(event_type AS BINARY) = CAST('audit.operation' AS BINARY)
        ),
        requested_events AS (
            SELECT CAST(LOWER(event_id) AS BINARY) AS event_key FROM outbox_records
            WHERE CAST(LOWER(request_id) AS BINARY) = CAST('{request_id}' AS BINARY)
            UNION
            SELECT CAST(LOWER(event_id) AS BINARY) FROM sys_oper_log
            WHERE CAST(LOWER(request_id) AS BINARY) = CAST('{request_id}' AS BINARY)
        ),
        candidate_audits AS (
            SELECT 'outbox' AS origin, event_id, request_id, created_at AS observed_at,
                SHA2(CONVERT(oper_url USING utf8mb4),256) AS path_sha256,
                SHA2(CONVERT(oper_name USING utf8mb4),256) AS actor_sha256,
                IF(CAST(tenant_id AS BINARY) = CAST('system' AS BINARY)
                    AND CAST(payload_tenant AS BINARY) = CAST('system' AS BINARY)
                    AND CAST(aggregate_type AS BINARY) = CAST('operation_audit' AS BINARY)
                    AND CAST(aggregate_id AS BINARY) = CAST(event_id AS BINARY)
                    AND CAST(dedupe_key AS BINARY) = CAST(event_id AS BINARY)
                    AND CAST(delivery AS BINARY) IN ('pending','running','published','dead')
                    AND CAST(operation_status AS BINARY) = CAST('success' AS BINARY)
                    AND CAST(request_method AS BINARY) = CAST('DELETE' AS BINARY)
                    AND CAST(operation_method AS BINARY) = CAST(CONCAT('DELETE ',oper_url) AS BINARY)
                    AND CAST(business_type AS BINARY) = CAST('FORCE_LOGOUT' AS BINARY), 1, 0) AS valid
            FROM outbox_records
            WHERE CAST(LOWER(request_id) AS BINARY) = CAST('{request_id}' AS BINARY)
                OR CAST(LOWER(event_id) AS BINARY) IN (SELECT event_key FROM requested_events)
                OR CAST(LOWER(aggregate_id) AS BINARY) IN (SELECT event_key FROM requested_events)
            UNION ALL
            SELECT 'projection', event_id, request_id, oper_time,
                SHA2(CONVERT(oper_url USING utf8mb4),256),
                SHA2(CONVERT(oper_name USING utf8mb4),256),
                IF(CAST(tenant_id AS BINARY) = CAST('system' AS BINARY)
                    AND CAST(status AS BINARY) = CAST('1' AS BINARY)
                    AND CAST(request_method AS BINARY) = CAST('DELETE' AS BINARY)
                    AND CAST(method AS BINARY) = CAST(CONCAT('DELETE ',oper_url) AS BINARY)
                    AND CAST(business_type AS BINARY) = CAST('FORCE_LOGOUT' AS BINARY), 1, 0)
            FROM sys_oper_log
            WHERE CAST(LOWER(request_id) AS BINARY) = CAST('{request_id}' AS BINARY)
                OR CAST(LOWER(event_id) AS BINARY) IN (SELECT event_key FROM requested_events)
        )
        SELECT origin,
            IF(OCTET_LENGTH(event_id)=36 AND REGEXP_LIKE(event_id,'{uuid_pattern}','c'),event_id,'-'),
            IF(OCTET_LENGTH(request_id)=36 AND REGEXP_LIKE(request_id,'{uuid_pattern}','c'),request_id,'-'),
            DATE_FORMAT(observed_at,'%Y-%m-%dT%H:%i:%sZ'), path_sha256, actor_sha256, valid
        FROM candidate_audits LIMIT 3;"
    )
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/post_process/identity_session_reconcile/audit.rs"]
mod tests;
