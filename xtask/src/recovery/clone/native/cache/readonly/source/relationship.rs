//! 重建三侧 successor 的全部绑定与碰撞投影，不接受收据自身摘要作为来源证明。
use super::*;
use crate::recovery::clone::native::{lifecycle::manifest, target_verify::input::request_review};
use std::{collections::BTreeSet, path::PathBuf};

const SIDES: [&str; 3] = ["seed", "base", "candidate"];

pub(in crate::recovery) fn load(backend: &Path, descriptor: &Value) -> Result<Value> {
    let value = document::read(&bound(backend, descriptor, "seed review successor")?)?;
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
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-clone-seed-review-successor"
        || value["remote_writes"].as_u64() != Some(0)
        || value["restore_qualified"] != false
    {
        return Err("seed review successor 结构无效".into());
    }
    if build(backend, &value)? != value {
        return Err("seed review successor 与当前绑定输入或关系摘要不一致".into());
    }
    bound(backend, descriptor, "seed review successor")?;
    Ok(value)
}

fn bound(backend: &Path, descriptor: &Value, label: &str) -> Result<PathBuf> {
    let path = common::bound_file(backend, descriptor, label)?;
    // 旧发布者用 Path 的规范拼写生成描述，不能改写同一文件的路径后重签关系。
    let normalized = path.components().collect::<PathBuf>();
    if descriptor["path"] != manifest::path_text(&normalized, label)? {
        return Err("successor 文件路径与原绑定不一致".into());
    }
    Ok(path)
}

pub(in crate::recovery) fn canonical_binding(backend: &Path, path: &Path) -> Result<Value> {
    let path = common::common_file(backend, path, "successor 输入文件")?;
    let file = common::binding(&path.components().collect::<PathBuf>())?;
    let value = document::read(&bound(backend, &file, "successor 输入文件")?)?;
    let mut descriptor = file;
    descriptor["canonical_sha256"] = json!(digest::plan_hash(&value)?);
    Ok(descriptor)
}

pub(in crate::recovery) fn build(backend: &Path, inputs: &Value) -> Result<Value> {
    input::name(document::text(&inputs["id"], "successor ID")?)?;
    let source = document::read(&bound(
        backend,
        &inputs["source_result"],
        "successor 来源结果",
    )?)?;
    qualified(&source)?;
    if source["status"] != "seed_source_registered" {
        return Err("successor 来源结果尚未登记".into());
    }
    let registration = document::read(&bound(
        backend,
        &source["registration"],
        "successor 来源登记",
    )?)?;
    if registration["kind"] != "devex-clone-seed-source-registration" {
        return Err("successor 来源登记类型无效".into());
    }
    let previous = read_canonical(backend, &inputs["predecessor_review"])?;
    predecessor(backend, &previous)?;
    let target = read_canonical(backend, &inputs["predecessor_request"])?;
    target_verify::binding_request::validate(backend, &target, &inputs["predecessor_review"])?;
    if target["side"] != "seed" {
        return Err("successor 历史请求必须属于 seed 侧".into());
    }
    let current = read_canonical(backend, &inputs["successor_review"])?;
    ready(backend, &current, true)?;
    requests(backend, inputs)?;
    let collision_projection = projection(&previous, &current)?;
    // 各输入绑定最后再次读取，不能在检查不同侧时替换早先已经校验的文件。
    for field in [
        "predecessor_review",
        "predecessor_request",
        "successor_review",
    ] {
        read_canonical(backend, &inputs[field])?;
    }
    for side in SIDES {
        read_canonical(backend, &inputs["requests"][side])?;
    }
    common::bound_file(backend, &inputs["source_result"], "successor 来源结果")?;
    common::bound_file(backend, &source["registration"], "successor 来源登记")?;
    let mut value = json!({
        "format_version":1, "kind":"devex-clone-seed-review-successor",
        "id":inputs["id"], "source_result":inputs["source_result"],
        "source_registration":source["registration"], "predecessor_review":inputs["predecessor_review"],
        "predecessor_request":inputs["predecessor_request"], "successor_review":inputs["successor_review"],
        "requests":inputs["requests"], "collision_projection":collision_projection,
        "remote_writes":0, "restore_qualified":false,
    });
    value["relationship_sha256"] = json!(digest::plan_hash(&value)?);
    Ok(value)
}

fn read_canonical(backend: &Path, descriptor: &Value) -> Result<Value> {
    // 保持四字段证据中的完整原路径，而其字节与规范摘要仍由共同读侧校验。
    let path = Path::new(document::text(&descriptor["path"], "successor 规范输入")?);
    if descriptor["path"]
        != manifest::path_text(
            &path.components().collect::<PathBuf>(),
            "successor 规范输入",
        )?
    {
        return Err("successor 文件路径与原绑定不一致".into());
    }
    canonical(backend, descriptor)
}

fn pending(review: &Value) -> Result<()> {
    if review["ready_for_execution"] != false {
        return Err("predecessor review 必须保持 pending".into());
    }
    let mut structural = review.clone();
    structural["ready_for_execution"] = json!(true);
    target_verify::input::request_review::validate(&structural)
}

fn predecessor(backend: &Path, review: &Value) -> Result<()> {
    match review["ready_for_execution"].as_bool() {
        Some(false) => pending(review),
        Some(true) => ready(backend, review, false),
        None => Err("predecessor review 必须有明确布尔就绪状态".into()),
    }
}

fn ready(backend: &Path, review: &Value, require_pending_parent: bool) -> Result<()> {
    let descriptor = &review["preflight"]["supersedes"];
    let previous = document::read(&bound(backend, descriptor, "successor preflight 前驱")?)?;
    if require_pending_parent {
        pending(&previous)?;
    }
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

#[cfg(test)]
#[test]
fn historical_ready_predecessor_verifies_both_preflight_lineages() {
    use crate::recovery::fixture_control::environment::protocol::tests as fixtures;

    let backend = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let local = backend.join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    let pending_path = directory.path().join("pending.json");
    let ready_path = directory.path().join("ready.json");
    let mut draft = fixtures::review(backend);
    draft["ready_for_execution"] = json!(false);
    for role in ["mysql", "mysqldump", "rustfs"] {
        draft["tools"][role]["sha256"] = json!("a".repeat(64));
    }
    document::write_new(&pending_path, &draft).unwrap();

    let mut original = fixtures::ready(draft);
    original["preflight"]["supersedes"] = common::binding(&pending_path).unwrap();
    predecessor(backend, &original).unwrap();
    ready(backend, &original, true).unwrap();
    document::write_new(&ready_path, &original).unwrap();

    let mut renewed = original;
    renewed["preflight"]["supersedes"] = common::binding(&ready_path).unwrap();
    renewed["tools"]["mysql"]["sha256"] = json!("b".repeat(64));
    renewed["preflight"]["tools"]["mysql"] = renewed["tools"]["mysql"].clone();
    predecessor(backend, &renewed).unwrap();
    assert!(ready(backend, &renewed, true).is_err());

    let mut drift = renewed.clone();
    drift["preflight"]["tools"]["mysql"]["sha256"] = json!("different");
    assert!(predecessor(backend, &drift).is_err());
    std::fs::write(ready_path, b"{}").unwrap();
    assert!(predecessor(backend, &renewed).is_err());
}

fn requests(backend: &Path, value: &Value) -> Result<()> {
    document::exact(&value["requests"], &SIDES, "successor 三侧请求")?;
    let mut generations = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for side in SIDES {
        let request = read_canonical(backend, &value["requests"][side])?;
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
    let before = side_projection(previous, "predecessor", &SIDES)?;
    let after = side_projection(current, "successor", &SIDES)?;
    require_disjoint(&before, &after)?;
    Ok(json!({"predecessor":before,"successor":after}))
}

/// 调用方先核验独立目标引用的审阅文件绑定，再对照原关系的两套保留资源。
pub(in crate::recovery) fn require_independent_b1_disjoint(
    backend: &Path,
    descriptor: &Value,
    review: &Value,
) -> Result<Value> {
    if request_review::Kind::read(review)? != request_review::Kind::B1RuntimeMeasurement {
        return Err("独立 B1 ARM 必须使用单侧测量审阅".into());
    }
    request_review::validate(review)?;
    let original = load(backend, descriptor)?;
    let candidate = side_projection(review, "measurement", &["candidate"])?;
    for role in ["predecessor", "successor"] {
        require_disjoint(&original["collision_projection"][role], &candidate)?;
    }
    Ok(candidate)
}

fn require_disjoint(before: &Value, after: &Value) -> Result<()> {
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
    Ok(())
}

fn side_projection(review: &Value, label: &str, sides: &[&str]) -> Result<Value> {
    let mut result =
        json!({"scopes":[],"databases":[],"redis":[],"objects":[],"paths":[],"sockets":[]});
    for &side in sides {
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
        url.port().ok_or("碰撞端点没有端口")?
    } else {
        http_port(text, &url, &host)?
    };
    Ok(format!("{host}:{port}"))
}

fn http_port(text: &str, url: &url::Url, host: &str) -> Result<u16> {
    let message = "successor HTTP 端点必须是明确的本机 loopback";
    let literal = match host {
        "127.0.0.1" => "127.0.0.1",
        "::1" => "[::1]",
        _ => return Err(message.into()),
    };
    let default = match url.scheme() {
        "http" => 80,
        "https" => 443,
        _ => return Err(message.into()),
    };
    // URL 的浏览器语义会重写简写 IP 与反斜线；原审阅只接受明确的回环主机。
    let text = text.replace(['\r', '\n', '\t'], "");
    let authority = text
        .split_once("://")
        .ok_or(message)?
        .1
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or_default();
    let literal_port = authority
        .strip_prefix(literal)
        .and_then(|tail| tail.strip_prefix(':'))
        .is_some_and(|port| port.bytes().all(|byte| byte.is_ascii_digit()));
    if (authority != literal && !literal_port)
        || !url.username().is_empty()
        || [url.password(), url.query(), url.fragment()]
            .into_iter()
            .flatten()
            .any(|value| !value.is_empty())
    {
        return Err(message.into());
    }
    Ok(url.port().filter(|port| *port != 0).unwrap_or(default))
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
