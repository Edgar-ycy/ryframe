use super::{
    batch, documents,
    model::{self, Inputs},
    session::{self, Client},
};
use crate::{Result, check::consumer_context::python};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, process::Stdio, time::Duration};
use uuid::{Uuid, Variant};

pub(in crate::recovery) struct Counts {
    pub tenants: u64,
    pub posts: u64,
    pub files: u64,
    pub subjects: Vec<Value>,
    pub session_subjects: Vec<Value>,
}

pub(in crate::recovery) struct Options {
    pub network: u8,
    pub request_interval_ms: u64,
    pub collect_subjects: bool,
}

pub(super) fn ownership(inputs: &Inputs) -> Result<()> {
    let request = json!({"format_version":1,"kind":"ryframe-xtask-recovery-reference","request":{"backend_dir":inputs.root,"format_version":1,"operation":"check-existing","plan":inputs.plan_path,"side":inputs.side,"write":false}});
    let tool = crate::rate_limit::current_tool_binding()?;
    let program = tool["path"].as_str().ok_or("原生协调器路径缺失")?;
    let payload = json!({"operation":"reference-readonly",
        "arguments":{"raw":serde_json::to_string(&request)?},"write":false});
    let mut command = crate::process::child_command(program);
    command
        .args([crate::recovery::source::evidence::MARKER])
        .current_dir(&inputs.root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = python::output(
        &inputs.root,
        batch::scrub(command),
        Some(&serde_json::to_vec(&payload)?),
        Duration::from_secs(60),
    )?;
    if crate::rate_limit::current_tool_binding()? != tool {
        return Err("原生协调器绑定在 ownership 检查期间变化".into());
    }
    if !output.status.success() {
        return Err("已有数据ownership或运行进程核验失败；没有业务写入，请检查参考阶段收据".into());
    }
    let response =
        documents::parse_fragment(std::str::from_utf8(&output.stdout)?, "ownership核验结果")?;
    if response["ok"] != true {
        return Err("原生 ownership 观察拒绝；保留本地检查收据".into());
    }
    let result = &response["result"]["value"];
    if result["plan_sha256"] != model::hash(&inputs.plan)?
        || result["side"] != inputs.side
        || result["scope_id"] != inputs.plan[&inputs.side]["scope_id"]
    {
        return Err("参考计划或核验侧在ownership检查期间变化".into());
    }
    inputs.unchanged()
}
pub(super) async fn run(inputs: &Inputs) -> Result<Value> {
    let dataset = inputs.dataset.as_ref().ok_or("已有数据收据缺失")?;
    let catalog = session::catalog(&inputs.root)?;
    if catalog
        .get("get_system_posts_by_id")
        .is_none_or(|v| v["method"] != "GET")
    {
        return Err("旧岗位验收必须使用只读查询契约".into());
    }
    let counts = verify_at(
        &inputs.plan[&inputs.side],
        dataset["tenants"].as_array().ok_or("租户样本缺失")?,
        &catalog,
        &inputs.password,
        Options {
            network: if inputs.side == "source" { 18 } else { 19 },
            request_interval_ms: inputs.plan["dataset"]["request_interval_ms"]
                .as_u64()
                .ok_or("请求节奏缺失")?,
            collect_subjects: false,
        },
        || inputs.unchanged(),
    )
    .await?;
    Ok(
        json!({"format_version":1,"status":"existing_data_verified","side":inputs.side,"scope_id":inputs.plan[&inputs.side]["scope_id"],"plan_sha256":model::hash(&inputs.plan)?,"source_scope_id":dataset["source_scope_id"],"dataset_sha256":inputs.dataset_sha()?,"actions":{"business":"read_only","objects":"read_only","session":"login_logout"},"restore_success":false,"tenants":counts.tenants,"posts":counts.posts,"files":counts.files}),
    )
}

pub(in crate::recovery) async fn verify_at(
    target: &Value,
    identities: &[Value],
    catalog: &std::collections::BTreeMap<String, Value>,
    password: &str,
    options: Options,
    checkpoint: impl FnMut() -> Result<()>,
) -> Result<Counts> {
    verify_at_with_sessions(
        target, identities, catalog, password, options, false, checkpoint,
    )
    .await
}

pub(in crate::recovery) async fn verify_at_with_sessions(
    target: &Value,
    identities: &[Value],
    catalog: &std::collections::BTreeMap<String, Value>,
    password: &str,
    options: Options,
    trusted_warm: bool,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<Counts> {
    if identities.len() != 11
        || trusted_warm && !options.collect_subjects
        || catalog
            .get("get_system_posts_by_id")
            .is_none_or(|value| value["method"] != "GET")
    {
        return Err("已有数据必须绑定十一租户及只读岗位查询契约".into());
    }
    let mut result = Counts {
        tenants: 0,
        posts: 0,
        files: 0,
        subjects: Vec::new(),
        session_subjects: Vec::new(),
    };
    for (index, identity) in identities.iter().enumerate() {
        checkpoint()?;
        let mut client = Client::at(
            target,
            json!({"tenant_id":identity["tenant_id"],"username":identity["username"]}),
            options.network,
            index,
            catalog,
            password,
            options.request_interval_ms,
        )?;
        client.login().await?;
        let verified: Result<_> = async {
            let subject = options
                .collect_subjects
                .then(|| access_subject(client.token()?, identity, trusted_warm))
                .transpose()?;
            Ok((verify_identity(&mut client, identity).await?, subject))
        }
        .await;
        if options.collect_subjects {
            // 来源验收只用刷新 Cookie 注销，避免生成访问令牌撤销键。
            client.clear_bearer();
        }
        let logout = client
            .request(&json!({"operation":"post_auth_logout"}))
            .await;
        let (counts, subject) = match (verified, logout) {
            (Ok(counts), Ok(_)) => counts,
            (Err(_), Err(_)) => return Err("已有数据验证及注销均失败；不发布验证成功".into()),
            (Err(error), _) | (_, Err(error)) => return Err(error),
        };
        if let Some((actor, session)) = subject {
            result.subjects.push(actor);
            result.session_subjects.extend(session);
        }
        result.tenants += 1;
        result.posts += counts.0;
        result.files += counts.1;
    }
    if trusted_warm
        && result
            .session_subjects
            .iter()
            .map(|session| session["sid_sha256"].as_str())
            .collect::<BTreeSet<_>>()
            .len()
            != 11
    {
        return Err("来源验收未取得十一项唯一会话摘要".into());
    }
    Ok(result)
}

fn access_subject(
    token: &str,
    identity: &Value,
    trusted_warm: bool,
) -> Result<(Value, Option<Value>)> {
    let encoded = token
        .split('.')
        .nth(1)
        .filter(|_| token.split('.').count() == 3)
        .ok_or("来源验收访问令牌声明无效")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "来源验收访问令牌声明无效")?;
    let claims: Value = serde_json::from_slice(&bytes).map_err(|_| "来源验收访问令牌声明无效")?;
    let user = claims["sub"]
        .as_str()
        .filter(|value| model::positive_id(&json!(value)).is_ok())
        .ok_or("来源验收访问令牌没有绑定当前租户用户")?;
    let version = claims["user_authorization_version"]
        .as_u64()
        .filter(|value| *value <= i32::MAX as u64)
        .ok_or("来源验收访问令牌没有绑定当前租户用户")?;
    if claims["token_type"] != "access"
        || claims["tenant_id"] != identity["tenant_id"]
        || claims["username"] != identity["username"]
    {
        return Err("来源验收访问令牌没有绑定当前租户用户".into());
    }
    let actor = json!({"tenant_id":identity["tenant_id"],"user_id":user,"user_authorization_version":version});
    let session = if trusted_warm {
        let sid = claims["sid"]
            .as_str()
            .filter(|sid| {
                sid.strip_prefix("s-").is_some_and(|value| {
                    Uuid::parse_str(value).is_ok_and(|uuid| {
                        uuid.get_version_num() == 4
                            && uuid.get_variant() == Variant::RFC4122
                            && uuid.hyphenated().to_string() == value
                    })
                })
            })
            .ok_or("来源验收访问令牌的会话标识不是规范 UUIDv4")?;
        Some(json!({"tenant_id":identity["tenant_id"],"user_id":user,
                    "sid_sha256":hex::encode(Sha256::digest(sid.as_bytes()))}))
    } else {
        None
    };
    Ok((actor, session))
}
async fn verify_identity(client: &mut Client, identity: &Value) -> Result<(u64, u64)> {
    let posts = identity["posts"].as_array().ok_or("岗位样本缺失")?;
    for post in posts {
        let response = client
            .request(&json!({"operation":"get_system_posts_by_id","path":{"id":post["id"]}}))
            .await?;
        if response["data"]["code"] != post["code"] || response["data"]["name"] != post["name"] {
            return Err("已有岗位内容不一致".into());
        }
    }
    let files = identity["files"].as_array().ok_or("对象样本缺失")?;
    for file in files {
        client.download(file).await?;
    }
    Ok((posts.len() as u64, files.len() as u64))
}
