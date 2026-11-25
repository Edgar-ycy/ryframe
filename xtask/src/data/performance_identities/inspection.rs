//! 复用共享运行来源核验；身份数据库检查全部使用原生只读连接。
use super::plan::{self, array, text};
use crate::{
    Result,
    devex::{
        job_evidence::protocol::Session,
        runtime_native::{mysql::Mysql, process::Cancellation},
    },
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[path = "inspection/user_create.rs"]
mod user_create;
pub(super) use user_create::{observe_activated_user, observe_created_user};

pub(super) const QUOTA_PHASE: &str = "quota";

pub(super) fn inspect(plan: &Value, phase: &str) -> Result<(Value, PathBuf)> {
    if !matches!(phase, "apply" | "verify" | QUOTA_PHASE) {
        return Err("身份环境检查阶段无效".into());
    }
    let backend = Path::new(text(&plan["environment"], "backend_dir")?);
    let private = plan::load(plan, backend)?;
    let environment = &private;
    let proof = runtime_proof(environment)?;
    let directory = PathBuf::from(text(&proof["runtime"], "api_config_dir")?);
    let table = crate::rate_limit::load_app_table(backend, Some(&directory), "test")
        .map_err(|error| error.message().to_owned())?;
    let table = serde_json::to_value(table)?;
    bindings(environment, &table)?;
    let database = &environment["database"];
    let mut control = open(database, &database["control"])?;
    let result: Result<Vec<Value>> = (|| {
        ownership(&mut control, &database["control"], environment, "control")?;
        let mut rows = Vec::new();
        for group in array(&plan["groups"])? {
            let found = control
                .query(&quota_sql(
                    text(&database["control"], "database")?,
                    text(group, "tenant_id")?,
                ))
                .map_err(|_| "身份租户配额只读查询失败")?;
            let [row] = found.as_slice() else {
                return Err("身份租户配额记录缺失或重复".into());
            };
            quota(row, group, environment, phase)?;
            rows.push(json!({"slot":group["slot"],"tenant":row}));
        }
        Ok(rows)
    })();
    let closed = control.close().map_err(|_| "身份控制库只读会话关闭失败");
    let tenants = result?;
    closed?;
    for target in array(&database["targets"])? {
        let mut session = open(database, &target["connection"])?;
        let result = ownership(
            &mut session,
            &target["connection"],
            environment,
            "tenant-data",
        );
        let closed = session.close().map_err(|_| "身份目标库只读会话关闭失败");
        result?;
        closed?;
    }
    if private != plan::load(plan, backend)? {
        return Err("身份环境检查期间来源变化".into());
    }
    Ok((
        json!({
            "scope_id": environment["scope_id"],
            "server_uuid": database["server_uuid"],
            "api_process": proof["api_process"],
            "tenants": tenants
        }),
        directory,
    ))
}
fn runtime_proof(environment: &Value) -> Result<Value> {
    let backend = Path::new(text(environment, "backend_dir")?);
    let request = json!({
        "backend": backend,
        "directory": environment["runtime"]["directory"],
        "scope_id": environment["scope_id"],
        "api_url": environment["api_url"]
    });
    crate::recovery::clone::native::post_process::runtime_proof(&request)
}
fn open(database: &Value, control: &Value) -> Result<Mysql> {
    let mut session = Mysql::open(
        &json!({"control":control,"mysql_client":database["mysql_client"]}),
        Cancellation::default(),
        Duration::from_secs(15),
    )
    .map_err(|_| "身份只读 MySQL 连接失败；检查绑定文件的连接、客户端及 TLS")?;
    session
        .query("SET SESSION TRANSACTION READ ONLY; START TRANSACTION READ ONLY")
        .map_err(|_| "身份只读事务未建立")?;
    Ok(session)
}
fn ownership(
    session: &mut impl Session,
    connection: &Value,
    environment: &Value,
    kind: &str,
) -> Result<()> {
    let schema = text(connection, "database")?;
    let scope = text(environment, "scope_id")?;
    let statement = format!(
        "SELECT JSON_OBJECT('server_uuid',@@server_uuid,'scope_id',scope_id,'marker',marker)
         FROM `{schema}`.ryframe_resource_ownership WHERE resource_kind='{kind}'"
    );
    let rows = session
        .query(&statement)
        .map_err(|_| "身份资源 ownership 读取失败")?;
    let [row] = rows.as_slice() else {
        return Err("身份资源 ownership 缺失或重复".into());
    };
    if row["server_uuid"] != environment["database"]["server_uuid"]
        || row["scope_id"] != scope
        || row["marker"] != format!("ryframe-owner:v1:{scope}:{kind}")
    {
        return Err("身份资源 ownership 或服务器 UUID 不匹配".into());
    }
    Ok(())
}
pub(super) fn bindings(environment: &Value, table: &Value) -> Result<()> {
    if table["scope_id"] != environment["scope_id"] {
        return Err("身份 scope 与文件配置不同".into());
    }
    let control = &environment["database"]["control"];
    for key in [
        "host", "port", "database", "username", "password", "tls_mode",
    ] {
        if control[key] != table["database"]["primary"][key] {
            return Err("身份控制库连接与已绑定运行配置不同".into());
        }
    }
    let frontend = url::Url::parse(text(environment, "frontend_url")?)?
        .origin()
        .ascii_serialization();
    if table["cors"]["allow_origins"] != json!([frontend]) {
        return Err("身份前端原点与已绑定 CORS 配置不同".into());
    }
    let empty = json!([]);
    let targets = array(table["tenant_data"].get("targets").unwrap_or(&empty))?;
    for declared in array(&environment["database"]["targets"])? {
        let actual = targets
            .iter()
            .find(|target| target["key"] == declared["key"]);
        if declared["key"] == "shared-control" {
            if declared["mode"] != "shared"
                || declared["connection"] != *control
                || actual
                    .is_some_and(|target| target["kind"] != "control" || target["mode"] != "shared")
            {
                return Err("身份控制库目标配置不匹配".into());
            }
            continue;
        }
        let actual = actual.ok_or("身份目标未登记在运行配置中")?;
        if actual["kind"] != "mysql" || actual["mode"] != declared["mode"] {
            return Err("身份目标模式或类型不同".into());
        }
        for key in [
            "host", "port", "database", "username", "password", "tls_mode",
        ] {
            if actual[key] != declared["connection"][key] {
                return Err("身份目标连接与运行配置不同".into());
            }
        }
        if ["tls_ca", "tls_client_cert", "tls_client_key"]
            .iter()
            .any(|key| actual[*key].as_str().is_some_and(|value| !value.is_empty()))
        {
            return Err("身份目标尚不接受未绑定的独立 TLS 文件".into());
        }
    }
    Ok(())
}
fn quota_sql(schema: &str, tenant: &str) -> String {
    format!(
        "SELECT JSON_OBJECT(
            'tenant_id',t.tenant_id,'status',t.status,
            'max_users',t.max_users,'max_roles',t.max_roles,
            'users',(SELECT COUNT(*) FROM `{schema}`.sys_user u
                WHERE u.tenant_id=t.tenant_id AND u.del_flag='0'),
            'roles',(SELECT COUNT(*) FROM `{schema}`.sys_role r
                WHERE r.tenant_id=t.tenant_id AND r.del_flag='0'),
            'target_key',p.current_target_key,'placement_state',p.state)
         FROM `{schema}`.sys_tenant t
         LEFT JOIN `{schema}`.sys_tenant_data_placement p ON p.tenant_id=t.tenant_id
         WHERE t.tenant_id='{tenant}'"
    )
}
pub(super) fn quota(row: &Value, group: &Value, environment: &Value, phase: &str) -> Result<()> {
    let quotas = &environment["quota"];
    let kind = text(group, "kind")?;
    let maximum = quotas[format!("{kind}_max_users")]
        .as_u64()
        .ok_or("用户配额配置无效")?;
    let required = if phase == "apply" {
        group["count"].as_u64().ok_or("身份组计数无效")?
    } else {
        0
    } + if kind == "tenant" {
        quotas["import_headroom_per_tenant"]
            .as_u64()
            .ok_or("导入余量无效")?
    } else {
        0
    };
    let users = row["users"].as_u64().ok_or("实际用户计数无效")?;
    let roles = row["roles"].as_u64().ok_or("实际角色计数无效")?;
    let role_max = row["max_roles"].as_u64().ok_or("实际角色配额无效")?;
    let current_maximum = row["max_users"].as_u64();
    let limits_match = if phase == QUOTA_PHASE {
        // 配额流程自己对照真实前像与计划目标，仅在已确认的应用阶段增加配额。
        current_maximum.is_some_and(|value| value > 0)
    } else {
        current_maximum == Some(maximum)
    };
    if row["tenant_id"] != group["tenant_id"]
        || row["status"] != "enabled"
        || !limits_match
        || maximum
            .checked_sub(users)
            .is_none_or(|space| space < required)
        || (phase == "apply" && role_max > 0 && roles >= role_max)
    {
        return Err("实际租户、用户或角色配额与计划不同".into());
    }
    if kind == "tenant" {
        let tenant = array(&environment["tenants"])?
            .iter()
            .find(|tenant| tenant["slot"] == group["slot"])
            .ok_or("租户放置映射缺失")?;
        if row["target_key"] != tenant["target_key"] || row["placement_state"] != "active" {
            return Err("租户放置目标或状态不同".into());
        }
    }
    Ok(())
}
