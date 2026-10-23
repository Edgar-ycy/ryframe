//! 夹具运行配置的纯构造；服务目标来自显式审阅与已解析凭据，不读取环境。
use super::protocol::check;
use crate::{Result, recovery::clone::native::json as native_json};
use serde_json::{Value, json};
use url::Url;

pub(super) fn settings(args: &Value) -> Result<Value> {
    native_json::exact(
        args,
        &["selected", "client", "values", "side"],
        "夹具配置构造",
    )?;
    let selected = &args["selected"];
    let client = &args["client"];
    let values = &args["values"];
    super::guards::reset_password(values)?;
    let api = endpoint(&selected["api_url"], false)?;
    let worker = endpoint(&selected["worker_ready_url"], false)?;
    let _frontend = endpoint(&selected["frontend_url"], true)?;
    let frontend_origin = selected["frontend_url"]
        .as_str()
        .ok_or("前端地址缺失")?
        .trim_end_matches('/');
    let databases = selected["databases"].as_array().ok_or("数据库目标缺失")?;
    let lookup = |key: &str| -> Result<&Value> {
        let entries = databases
            .iter()
            .filter(|item| item["key"] == key)
            .collect::<Vec<_>>();
        check(entries.len() == 1, "夹具数据库目标缺失或重复")?;
        Ok(entries[0])
    };
    let tls = client["tls_mode"].as_str().ok_or("MySQL TLS 模式缺失")?;
    let targets = ["shared", "dedicated-a", "dedicated-b"]
        .iter()
        .map(|key| {
            let item = lookup(key)?;
            Ok(
                json!({"key":key,"kind":"mysql","mode":item["mode"],"host":item["host"],
            "port":item["port"],"database":item["database"],"tls_mode":tls}),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let scope = selected["scope_id"].as_str().ok_or("scope_id 缺失")?;
    let redis = Url::parse(selected["redis"]["url"].as_str().ok_or("Redis URL 缺失")?)?;
    let port = match &client["port"] {
        Value::String(text) => text.parse::<u16>()?,
        value => u16::try_from(value.as_u64().ok_or("MySQL 端口必须是整数")?)?,
    };
    let settings = json!({
        "scope_id":scope,"snowflake_worker_id":3,
        "app":{"host":host(&api.0)?,"port":api.1},
        "multi_tenancy":{"enabled":true},"api_docs":{"enabled":false},
        "bootstrap":{"admin_password":values["admin_password"]},
        "cors":{"allow_origins":[frontend_origin]},
        "database.primary":{"host":client["host"],"port":port,"database":lookup("shared-control")?["database"],"tls_mode":tls},
        "object_storage":{"backend":"rustfs","endpoint":selected["objects"]["endpoint"],
            "region":selected["objects"]["region"],"use_ssl":false,"access_key":values["access_key"],"secret_key":values["secret_key"]},
        "redis":{"mode":"required","host":"127.0.0.1","port":redis.port().unwrap_or(16390),
            "database":0,"tls":false,"password":values["redis_password"]},
        "auth":{"jwt_secret":values["jwt_secret"]},"monitor":{"metrics_bearer_token":values["metrics_token"]},
        "jobs":{"mode":"external","health_host":host(&worker.0)?,"health_port":worker.1,"scheduler_enabled":args["side"] == "seed"},
        "rate_limit":{"capacity":1000},
        "reset":{"credential_version":"fixture-v1","legacy_mysql_exclusive":true,"legacy_redis_exclusive":true,
            "legacy_object_storage_exclusive":true,"redis_outside_sentinel_key":format!("ryframe:devex-fresh:{scope}:sentinel")}
    });
    Ok(json!({"settings":settings,"targets":targets}))
}

fn host(url: &Url) -> Result<String> {
    Ok(url
        .host_str()
        .ok_or("夹具地址缺少主机")?
        .trim_matches(['[', ']'])
        .to_owned())
}
fn endpoint(value: &Value, origin_only: bool) -> Result<(Url, u16)> {
    let text = value.as_str().ok_or("夹具地址缺失")?;
    let url = Url::parse(text)?;
    // Url 会将显式默认端口归一化；因此从 authority 同时验证端口声明。
    let authority = text
        .split_once("://")
        .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
        .unwrap_or("");
    let explicit_port = authority
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok());
    check(
        matches!(host(&url)?.as_str(), "127.0.0.1" | "::1")
            && explicit_port.is_some()
            && matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && (!origin_only
                || (url.path() == "/" && url.query().is_none() && url.fragment().is_none())),
        "夹具 API、Worker 或前端地址必须是明确本机端口",
    )?;
    Ok((url, explicit_port.ok_or("夹具端口缺失")?))
}

pub(super) fn text(args: &Value) -> Result<Value> {
    native_json::exact(args, &["settings", "targets"], "夹具 TOML 构造")?;
    let settings = args["settings"].as_object().ok_or("夹具配置不是对象")?;
    let targets = args["targets"].as_array().ok_or("租户目标不是数组")?;
    check(
        !targets.is_empty(),
        "夹具测试配置必须声明至少一个租户数据目标",
    )?;
    let mut sections = Vec::new();
    sections.push(
        settings
            .iter()
            .filter(|(_, value)| !value.is_object())
            .map(|(key, value)| Ok(format!("{key} = {}", serde_json::to_string(value)?)))
            .collect::<Result<Vec<_>>>()?
            .join("\n"),
    );
    for (name, table) in settings.iter().filter(|(_, value)| value.is_object()) {
        let fields = table
            .as_object()
            .expect("已过滤表")
            .iter()
            .map(|(key, value)| Ok(format!("{key} = {}", serde_json::to_string(value)?)))
            .collect::<Result<Vec<_>>>()?;
        sections.push(format!("[{name}]\n{}", fields.join("\n")));
    }
    for target in targets {
        let key = target["key"].as_str().ok_or("租户目标键缺失")?;
        let mut fields = vec![
            format!("key = {}", serde_json::to_string(key)?),
            format!(
                "display_name = {}",
                serde_json::to_string(&format!("夹具 {key}"))?
            ),
        ];
        for field in ["kind", "mode", "host", "port", "database", "tls_mode"] {
            check(target.get(field).is_some(), "租户目标字段缺失")?;
            fields.push(format!(
                "{field} = {}",
                serde_json::to_string(&target[field])?
            ));
        }
        fields.push("max_connections = 8".into());
        sections.push(format!("[[tenant_data.targets]]\n{}", fields.join("\n")));
    }
    let text = format!("{}\n", sections.join("\n\n"));
    toml::from_str::<toml::Table>(&text)?;
    Ok(json!({"text":text}))
}

/// 当前凭据在准备配置时生成，初始化密码只用于首次受限登录。
pub(super) fn administrator(args: &Value) -> Result<Value> {
    native_json::exact(args, &["admin_password"], "夹具当前凭据构造")?;
    super::guards::reset_password(args)?;
    let initial = args["admin_password"].as_str().ok_or("初始化密码缺失")?;
    let password = loop {
        let candidate = format!(
            "Aa1!{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        super::validate_password(&candidate)?;
        if candidate != initial {
            break candidate;
        }
    };
    let text = toml::to_string(&toml::toml! {
        [administrator]
        tenant_id = "system"
        username = "admin"
        password = password
    })?;
    Ok(json!({"text": text}))
}
