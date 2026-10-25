//! 重建三侧 successor 的全部绑定与碰撞投影，不接受收据自身摘要作为来源证明。
use super::*;
use std::{collections::BTreeSet, path::PathBuf};

const SIDES: [&str; 3] = ["seed", "base", "candidate"];

pub(super) fn load(backend: &Path, descriptor: &Value) -> Result<Value> {
    let value = common::read_bound(backend, descriptor, "seed review successor")?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "id",
            "source_result",
            "source_registration",
            "predecessor_review",
            "predecessor_request",
            "successor_review",
            "requests",
            "collision_projection",
            "relationship_sha256",
            "remote_writes",
            "restore_qualified",
        ],
        "seed review successor",
    )?;
    input::name(document::text(&value["id"], "successor ID")?)?;
    let mut unhashed = value.clone();
    unhashed
        .as_object_mut()
        .ok_or("successor 不是对象")?
        .remove("relationship_sha256");
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-clone-seed-review-successor"
        || value["remote_writes"].as_u64() != Some(0)
        || value["restore_qualified"] != false
        || digest::plan_hash(&unhashed)? != value["relationship_sha256"]
    {
        return Err("seed review successor 结构或关系摘要无效".into());
    }
    let source = common::read_bound(backend, &value["source_result"], "successor 来源结果")?;
    qualified(&source)?;
    if source["status"] != "seed_source_registered"
        || source["registration"] != value["source_registration"]
    {
        return Err("successor 来源结果与内层登记不一致".into());
    }
    let registration = common::read_bound(backend, &source["registration"], "successor 来源登记")?;
    if registration["kind"] != "devex-clone-seed-source-registration" {
        return Err("successor 来源登记类型无效".into());
    }
    let previous = canonical(backend, &value["predecessor_review"])?;
    pending(&previous)?;
    let target = canonical(backend, &value["predecessor_request"])?;
    target_verify::binding_request::validate(backend, &target, &value["predecessor_review"])?;
    if target["side"] != "seed" {
        return Err("successor 历史请求必须属于 seed 侧".into());
    }
    let current = canonical(backend, &value["successor_review"])?;
    ready(backend, &current)?;
    requests(backend, &value)?;
    if projection(&previous, &current)? != value["collision_projection"] {
        return Err("successor 碰撞投影不同于完整输入重建结果".into());
    }
    // 各输入绑定最后再次读取，不能在检查不同侧时替换早先已经校验的文件。
    for field in [
        "predecessor_review",
        "predecessor_request",
        "successor_review",
    ] {
        canonical(backend, &value[field])?;
    }
    common::bound_file(backend, descriptor, "seed review successor")?;
    Ok(value)
}

fn pending(review: &Value) -> Result<()> {
    if review["ready_for_execution"] != false {
        return Err("predecessor review 必须保持 pending".into());
    }
    let mut structural = review.clone();
    structural["ready_for_execution"] = json!(true);
    target_verify::input::request_review::validate(&structural)
}

fn ready(backend: &Path, review: &Value) -> Result<()> {
    let descriptor = &review["preflight"]["supersedes"];
    let previous = common::read_bound(backend, descriptor, "successor preflight 前驱")?;
    pending(&previous)?;
    let (code, result) =
        crate::recovery::fixture_control::environment::protocol::execute(&serde_json::to_vec(
            &json!({"format_version":1,"operation":"preflight-successor",
            "arguments":{"previous":previous,"review":review},"write":false}),
        )?);
    if code != 0 || result["ok"] != true {
        return Err("successor review 与预检前驱语义不同".into());
    }
    common::bound_file(backend, descriptor, "successor preflight 前驱")?;
    Ok(())
}

fn requests(backend: &Path, value: &Value) -> Result<()> {
    document::exact(&value["requests"], &SIDES, "successor 三侧请求")?;
    let mut generations = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for side in SIDES {
        let request = canonical(backend, &value["requests"][side])?;
        target_verify::published::request_binding(backend, &request)?;
        if request["side"] != side || request["review"] != value["successor_review"] {
            return Err("successor 三侧请求未绑定同一 ready review 的对应侧".into());
        }
        generations.insert(digest::plan_hash(&request["storage"])?);
        ids.insert(document::text(&request["id"], "successor 请求 ID")?.to_owned());
    }
    if generations.len() != 1 || ids.len() != SIDES.len() {
        return Err("successor 三侧请求必须共享服务代次且使用不同 ID".into());
    }
    Ok(())
}

fn projection(previous: &Value, current: &Value) -> Result<Value> {
    let before = side_projection(previous, "predecessor")?;
    let after = side_projection(current, "successor")?;
    for field in ["scopes", "databases", "redis", "objects", "sockets"] {
        let existing = document::array(&before[field], "原碰撞投影")?
            .iter()
            .map(|v| document::text(&v["identity"], "资源身份"))
            .collect::<Result<BTreeSet<_>>>()?;
        for item in document::array(&after[field], "后继碰撞投影")? {
            if existing.contains(document::text(&item["identity"], "资源身份")?) {
                return Err("successor 资源与历史计划发生碰撞".into());
            }
        }
    }
    for before in document::array(&before["paths"], "原碰撞路径")? {
        for after in document::array(&after["paths"], "后继碰撞路径")? {
            let old = path_key(document::text(&before["identity"], "原路径")?);
            let new = path_key(document::text(&after["identity"], "后继路径")?);
            if old.starts_with(&new) || new.starts_with(&old) {
                return Err("successor 路径与历史计划重叠".into());
            }
        }
    }
    Ok(json!({"predecessor":before,"successor":after}))
}

fn side_projection(review: &Value, label: &str) -> Result<Value> {
    let mut result =
        json!({"scopes":[],"databases":[],"redis":[],"objects":[],"paths":[],"sockets":[]});
    for side in SIDES {
        let selected = &review["scopes"][side];
        let owner = format!("{label}.{side}");
        let scope = document::text(&selected["scope_id"], "资源 scope")?;
        push(
            &mut result,
            "scopes",
            json!({"owner":owner,"identity":scope}),
        )?;
        for database in document::array(&selected["databases"], "审阅数据库")? {
            let identity = format!(
                "{}:{}",
                document::text(&database["expected_server_uuid"], "数据库 UUID")?,
                document::text(&database["database"], "数据库名称")?.to_lowercase()
            );
            push(
                &mut result,
                "databases",
                json!({"owner":owner,"key":database["key"],"identity":identity}),
            )?;
        }
        let redis = &selected["redis"];
        if redis["namespace"] != format!("ryframe:{{{scope}}}:")
            || redis["ownership_key"] != format!("ryframe:{{{scope}}}:.ryframe-owner")
            || redis["ownership_value"] != format!("ryframe-owner:v1:{scope}:redis")
        {
            return Err("successor Redis ownership 投影无效".into());
        }
        push(
            &mut result,
            "redis",
            json!({"owner":owner,"identity":redis["ownership_key"]}),
        )?;
        for bucket in crate::recovery::clone::native::input::BUCKETS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
        {
            push(
                &mut result,
                "objects",
                json!({"owner":owner,"bucket":bucket,
                "identity":format!("{}|{scope}/", document::text(&selected["objects"]["endpoint"], "对象端点")?)}),
            )?;
        }
        for field in ["runtime_dir", "identity_ledger"] {
            add_path(&mut result, &owner, &selected[field])?;
        }
        for field in ["api_url", "worker_ready_url", "frontend_url"] {
            push(
                &mut result,
                "sockets",
                json!({"owner":format!("{owner}.{field}"),"identity":socket(&selected[field],false)?}),
            )?;
        }
    }
    add_services(&mut result, review, label)?;
    sort_projection(&mut result)?;
    Ok(result)
}

fn add_services(result: &mut Value, review: &Value, label: &str) -> Result<()> {
    let services = &review["services"];
    for (role, path) in [
        ("future", &review["future_root"]),
        ("rustfs", &services["rustfs"]["data_dir"]),
        ("redis", &services["redis"]["directory"]),
    ] {
        add_path(result, &format!("{label}.{role}"), path)?;
    }
    for (role, address, redis) in [
        ("rustfs-api", &services["rustfs"]["api"], false),
        ("rustfs-console", &services["rustfs"]["console"], false),
        ("redis", &services["redis"]["endpoint"], true),
    ] {
        push(
            result,
            "sockets",
            json!({"owner":format!("{label}.{role}"),"identity":socket(address,redis)?}),
        )?;
    }
    Ok(())
}

fn add_path(result: &mut Value, owner: &str, value: &Value) -> Result<()> {
    let path = Path::new(document::text(value, "碰撞路径")?);
    if !path.is_absolute() {
        return Err("successor 碰撞路径必须是绝对路径".into());
    }
    let normalized = path.components().collect::<PathBuf>();
    push(
        result,
        "paths",
        json!({"owner":owner,"identity":normalized}),
    )
}

fn socket(value: &Value, redis: bool) -> Result<String> {
    let text = document::text(value, "碰撞端点")?;
    let url = if redis && !text.contains("://") {
        format!("redis://{text}")
    } else {
        text.to_owned()
    };
    let url = url::Url::parse(&url).map_err(|_| "successor 碰撞端点无效")?;
    let host = match url.host().ok_or("碰撞端点没有主机")? {
        url::Host::Ipv4(host) => host.to_string(),
        url::Host::Ipv6(host) => host.to_string(),
        url::Host::Domain(host) => host.to_owned(),
    };
    if redis && (url.scheme() != "redis" || !matches!(host.as_str(), "127.0.0.1" | "::1")) {
        return Err("successor Redis 端点必须是明确 loopback".into());
    }
    let port = if redis {
        url.port()
    } else {
        url.port_or_known_default()
    }
    .ok_or("碰撞端点没有端口")?;
    Ok(format!("{host}:{port}"))
}

fn path_key(value: &str) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(value.to_lowercase())
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(value)
    }
}

fn push(result: &mut Value, field: &str, value: Value) -> Result<()> {
    result[field]
        .as_array_mut()
        .ok_or("碰撞投影不是数组")?
        .push(value);
    Ok(())
}

fn sort_projection(result: &mut Value) -> Result<()> {
    for field in [
        "scopes",
        "databases",
        "redis",
        "objects",
        "paths",
        "sockets",
    ] {
        result[field]
            .as_array_mut()
            .ok_or("碰撞投影不是数组")?
            .sort_by(|left, right| {
                ["identity", "owner", "key", "bucket"]
                    .iter()
                    .map(|field| left[*field].as_str().unwrap_or_default())
                    .cmp(
                        ["identity", "owner", "key", "bucket"]
                            .iter()
                            .map(|field| right[*field].as_str().unwrap_or_default()),
                    )
            });
    }
    Ok(())
}
