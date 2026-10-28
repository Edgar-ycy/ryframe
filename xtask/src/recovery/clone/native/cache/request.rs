//! 缓存请求与配置采集结果的纯模型；不读取环境覆盖，不回显凭据。
use super::super::json as doc;
use super::rejected;
use super::{markers, model};
use crate::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

const FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "side",
    "manifest",
    "initialized",
    "target_environment",
    "previous",
    "process_receipt",
    "process",
    "markers",
];
const REDIS_FIELDS: &[&str] = &[
    "port",
    "wsl",
    "distribution",
    "pid",
    "started",
    "executable",
    "sha256",
    "run_id",
    "configuration",
];

pub(super) fn bound_process(backend: &Path, arguments: &Value) -> Result<Value> {
    bound_process_with(backend, arguments, |memory| run("process-model", memory))
}

fn bound_process_with(
    backend: &Path,
    arguments: &Value,
    model: impl FnOnce(&Value) -> Result<Value>,
) -> Result<Value> {
    use super::super::resources::common;
    doc::exact(
        arguments,
        &[
            "request",
            "review",
            "receipt",
            "directory",
            "config",
            "options",
        ],
        "缓存绑定review模型",
    )?;
    let binding = &arguments["review"];
    doc::exact(binding, &["path", "bytes", "sha256"], "缓存review文件绑定")?;
    let path = common::bound_file(backend, binding, "缓存review")?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(doc::MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let review = review_bytes(binding, &bytes)?;
    common::bound_file(backend, binding, "解析后缓存review")?;
    let mut memory = arguments.clone();
    memory["review"] = review;
    let result = model(&memory);
    common::bound_file(backend, binding, "模型执行后缓存review")?;
    result
}

fn review_bytes(binding: &Value, bytes: &[u8]) -> Result<Value> {
    if bytes.len() as u64 > doc::MAX_JSON_BYTES
        || binding["bytes"].as_u64() != Some(bytes.len() as u64)
        || binding["sha256"].as_str() != Some(hex::encode(Sha256::digest(bytes)).as_str())
    {
        return Err(rejected("缓存review实际读取字节与绑定不一致"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "缓存review不是UTF-8 JSON")?;
    let value =
        doc::parse_fragment(text, "缓存review").map_err(|_| "缓存review不是唯一字段的严格JSON")?;
    if !value.is_object() {
        return Err(rejected("缓存review必须为对象"));
    }
    Ok(value)
}

pub(super) fn run(operation: &str, a: &Value) -> Result<Value> {
    match operation {
        "environment-select" => {
            doc::exact(a, &["private"], "缓存私有配置选择")?;
            private_environment(&a["private"])
        }
        "environment-model" => {
            doc::exact(a, &["request", "redis"], "缓存配置端点")?;
            environment(&a["request"], &a["redis"]).map(|()| Value::Null)
        }
        "request-head" => {
            doc::exact(a, &["request", "manifest", "value"], "缓存请求头")?;
            head(&a["request"], &a["manifest"], &a["value"]).map(|()| Value::Null)
        }
        "request-model" => {
            doc::exact(
                a,
                &["request", "initial", "original", "selected"],
                "缓存请求模型",
            )?;
            request(&a["request"], &a["initial"], &a["original"], &a["selected"])
                .map(|()| Value::Null)
        }
        "process-model" => {
            doc::exact(
                a,
                &[
                    "request",
                    "review",
                    "receipt",
                    "directory",
                    "config",
                    "options",
                ],
                "缓存进程绑定模型",
            )?;
            process(a).map(|()| Value::Null)
        }
        "successor-model" => {
            doc::exact(
                a,
                &[
                    "request",
                    "directory",
                    "value",
                    "source_directory",
                    "source_manifest",
                    "tools",
                    "tool_wsl_path",
                    "original_wsl_path",
                ],
                "缓存工具后继模型",
            )?;
            successor(a)
        }
        "pending-target" => {
            doc::exact(a, &["target", "seed_target"], "缓存初始化来源")?;
            if a["target"] != a["seed_target"] {
                return Err(rejected("缓存初始化不是 successor 冻结的原 seed"));
            }
            Ok(Value::Null)
        }
        _ => Err(rejected("未知缓存模型操作")),
    }
}
fn private_environment(private: &Value) -> Result<Value> {
    doc::exact(private, &["environment"], "缓存私有环境")?;
    let values = doc::object(&private["environment"], "缓存私有环境映射")?;
    let mut names = std::collections::BTreeSet::new();
    for (key, value) in values {
        let name = key.to_ascii_uppercase();
        if !matches!(name.as_str(), "APP_ENV" | "TEMP" | "TMP")
            || !names.insert(name)
            || !value
                .as_str()
                .is_some_and(|text| !text.is_empty() && !text.contains('\0'))
        {
            return Err(rejected(
                "缓存私有环境只允许唯一的 APP_ENV、TEMP、TMP 文本字段；请将连接与密码写入绑定配置文件",
            ));
        }
    }
    if values.get("APP_ENV") != Some(&json!("test")) {
        return Err(rejected(
            "缓存恢复必须明确选择 APP_ENV=test；请核对绑定的私有环境文件",
        ));
    }
    Ok(private["environment"].clone())
}

fn environment(request: &Value, redis: &Value) -> Result<()> {
    doc::object(redis, "缓存恢复配置文件的 redis 配置表")?;
    let expected =
        json!({"host":"127.0.0.1","port":request["previous"]["port"],"database":0,"tls":false});
    for (key, value) in expected.as_object().unwrap() {
        if redis.get(key) != Some(value) {
            return Err(rejected("缓存配置的 Redis 端点、类型或原目标不同"));
        }
    }
    model::password(&redis["password"])?;
    if request["process"]["password"] != redis["password"] {
        return Err(rejected(
            "缓存进程 password 与配置文件的 redis.password 不同",
        ));
    }
    Ok(())
}
fn head(request: &Value, manifest: &Value, value: &Value) -> Result<()> {
    doc::exact(request, FIELDS, "缓存恢复请求")?;
    if request["format_version"].as_u64() != Some(1)
        || request["kind"] != "devex-clone-cache-restart"
        || request["side"] != "target"
        || request["manifest"] != *manifest
        || request["initialized"] != value["initialized"]
        || request["target_environment"] != value["target_environment"]
    {
        return Err(rejected("缓存恢复必须属于固定运行的原目标和明确环境"));
    }
    doc::exact(&request["previous"], REDIS_FIELDS, "缓存原 Redis 代次")?;
    Ok(())
}
fn request(request: &Value, initial: &Value, original: &Value, selected: &Value) -> Result<()> {
    if request["previous"] != original["storage"]["redis"]
        || request["previous"] != initial["generation"]["storage"]["redis"]
    {
        return Err(rejected("缓存恢复原代次与完整初始化历史不符"));
    }
    let mut markers = json!({});
    for key in ["namespace", "ownership_key", "ownership_value"] {
        markers[key] = selected["redis"]
            .get(key)
            .ok_or("缓存初始化 Redis 标记缺失")?
            .clone();
    }
    for key in ["sentinel_key", "sentinel_value"] {
        markers[key] = original["reset"]
            .get(key)
            .ok_or("缓存初始化 sentinel 缺失")?
            .clone();
    }
    let scope = doc::text(&original["target"]["scope_id"], "缓存初始化 scope")?;
    let namespace = format!("ryframe:{{{scope}}}:");
    if request["markers"] != markers
        || markers["namespace"] != namespace
        || markers["ownership_key"] != format!("{namespace}.ryframe-owner")
        || doc::text(&markers["sentinel_key"], "缓存 sentinel")?.starts_with(&namespace)
        || markers["sentinel_key"] != format!("ryframe:devex-fresh:{scope}:sentinel")
        || markers["sentinel_value"]
            != format!(
                "devex-fresh:{}",
                doc::text(&original["id"], "缓存初始化 ID")?
            )
        || initial["redis"] != markers::images(&markers).1
        || selected["redis"]["url"]
            != format!(
                "redis://127.0.0.1:{}/0",
                request["previous"]["port"]
                    .as_u64()
                    .ok_or("缓存原端口无效")?
            )
    {
        return Err(rejected("只能原子恢复原初始化明确记录的两个验收标记"));
    }
    Ok(())
}
fn process(a: &Value) -> Result<()> {
    let previous = &a["request"]["previous"];
    let process = &a["request"]["process"];
    let receipt = &a["receipt"];
    let review = &a["review"];
    if receipt["format_version"].as_u64() != Some(1)
        || receipt["role"] != "redis"
        || receipt["lifecycle"] != "running"
        || receipt["request_storage"] != *previous
        || receipt["scope_id"] != review["services"]["redis"]["scope_id"]
    {
        return Err(rejected("原 Redis 进程收据与本次初始化或服务身份不同"));
    }
    let old = json!({"pid":previous["pid"],"started":previous["started"],"executable":previous["executable"]});
    doc::exact(
        &receipt["linux_identity"],
        &["pid", "started", "executable", "boot_id"],
        "原 Redis Linux 身份",
    )?;
    for key in ["pid", "started", "executable"] {
        if receipt["linux_identity"][key] != old[key] {
            return Err(rejected("原 Redis Linux 创建身份不符"));
        }
    }
    let directory = Path::new(doc::text(&a["directory"], "缓存实际目录")?);
    if directory
        != Path::new(doc::text(
            &review["services"]["redis"]["directory"],
            "缓存原目录",
        )?)
    {
        return Err(rejected("缓存恢复只能使用原服务物理目录"));
    }
    if Path::new(doc::text(&a["config"], "缓存配置路径")?) != directory.join("redis.conf") {
        return Err(rejected("缓存恢复配置不属于原目录"));
    }
    let mut expected = json!({"scope_id":receipt["scope_id"],"launcher":review["tools"]["redis_server"]["path"],"previous_identity":old,"previous_boot_id":receipt["linux_identity"]["boot_id"],"previous_run_id":previous["run_id"]});
    for key in [
        "wsl",
        "distribution",
        "executable",
        "sha256",
        "configuration",
        "port",
    ] {
        expected[key] = previous[key].clone();
    }
    if !super::stage::successor::contains(process, &expected) {
        return Err(rejected("缓存进程请求改变原服务、配置、工具或创建身份"));
    }
    let mut passwords = Vec::new();
    for row in doc::array(&a["options"], "Redis 配置参数")? {
        let row = doc::array(row, "Redis 配置行")?;
        if row.first() == Some(&json!("requirepass")) {
            passwords.push(json!(&row[1..]));
        }
    }
    if passwords != vec![json!([process["password"]])] {
        return Err(rejected("原 Redis 配置认证与绑定的 password 不同"));
    }
    Ok(())
}
fn successor(a: &Value) -> Result<Value> {
    if Path::new(doc::text(&a["source_directory"], "缓存来源目录")?)
        != Path::new(doc::text(&a["directory"], "缓存运行目录")?)
        || a["source_manifest"] != a["value"]
    {
        return Err(rejected("缓存工具后继必须属于当前已发布 seed 的原 run"));
    }
    let original = &a["request"]["process"];
    let tools = &a["tools"];
    if original["format_version"] == 2 {
        let next = super::native_tool::successor(original, tools)?;
        if next == *original {
            return Err(rejected("缓存工具没有变化，无需登记 successor"));
        }
        return Ok(next);
    }
    let redis = &tools["redis_server"];
    let python = &tools["redis_python"];
    if redis["distribution"] != original["distribution"]
        || redis["resolved_path"] != original["executable"]
        || redis["sha256"] != original["sha256"]
        || python["distribution"] != original["distribution"]
        || Path::new(doc::text(&a["tool_wsl_path"], "缓存后继入口")?)
            != Path::new(doc::text(&a["original_wsl_path"], "缓存原入口")?)
        || python["path"] != original["python"]["path"]
        || python["resolved_path"] != original["python"]["executable"]
    {
        return Err(rejected("缓存工具后继不得改变 Redis、发行版或解释器入口"));
    }
    let mut effective = original.clone();
    effective["wsl"] = tools["wsl"].clone();
    effective["python"] = json!({"path":python["path"],"executable":python["resolved_path"],"sha256":python["sha256"]});
    if effective == *original {
        return Err(rejected("缓存工具没有变化，无需登记 successor"));
    }
    Ok(effective)
}

#[cfg(test)]
mod bound_review_tests {
    use super::super::super::resources::common;
    use super::*;

    #[test]
    fn bound_review_rpc_keeps_full_sources_and_rejects_raw_review_or_any_file_drift() {
        let backend = crate::workspace::root_dir();
        std::fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let directory = tempfile::tempdir_in(backend.join(".local-tests")).unwrap();
        let file = directory.path().join("review.json");
        let mut sources =
            vec![json!({"path":"xtask/source-inventory.json","sha256":"a".repeat(64)})];
        for index in 0..5000 {
            sources.push(
                json!({"path":format!("xtask/src/fixture{index:05}.rs"),"sha256":"b".repeat(64)}),
            );
        }
        let review = json!({"tools":{"native_tool":{"sources":sources}},
            "preflight":{"tools":{"native_tool":{"sources":sources}}}});
        doc::write_new(&file, &review).unwrap();
        let mut rpc = json!({"request":{"process":{"native_tool":{"sources":sources}}},
            "review":common::binding(&file).unwrap(),"receipt":{},"directory":"D:/fixture","config":"D:/fixture/redis.conf","options":[]});
        let bytes = std::fs::read(&file).unwrap();
        assert_eq!(review_bytes(&rpc["review"], &bytes).unwrap(), review);
        let mut changed = bytes.clone();
        let index = changed.iter().position(|byte| *byte == b'a').unwrap();
        changed[index] = b'b';
        assert!(review_bytes(&rpc["review"], &changed).is_err());
        assert!(review_bytes(&rpc["review"], &bytes[..bytes.len() - 1]).is_err());
        let duplicate = br#"{"private-marker":1,"private-marker":2}"#;
        let binding =
            json!({"bytes":duplicate.len(),"sha256":hex::encode(Sha256::digest(duplicate))});
        assert!(
            !review_bytes(&binding, duplicate)
                .unwrap_err()
                .to_string()
                .contains("private-marker")
        );
        let mut raw = rpc.clone();
        raw["review"] = review.clone();
        assert!(serde_json::to_vec(&raw).unwrap().len() > 1024 * 1024);
        let bytes = serde_json::to_vec(&json!({"format_version":1,"kind":"ryframe-cache-process",
            "operation":"process-model","arguments":rpc}))
        .unwrap();
        assert!(bytes.len() < 1024 * 1024);
        assert!(super::super::execute(&backend,&serde_json::to_vec(&json!({
            "format_version":1,"kind":"ryframe-cache-process","operation":"process-model","arguments":raw})).unwrap()).is_err());
        bound_process_with(&backend, &rpc, |memory| {
            assert_eq!(memory["review"], review);
            Ok(Value::Null)
        })
        .unwrap();
        assert!(
            bound_process_with(&backend, &rpc, |memory| {
                assert_eq!(memory["review"], review);
                std::fs::write(&file, b"{\"changed\":true}").unwrap();
                Ok(Value::Null)
            })
            .is_err()
        );
        assert!(bound_process_with(&backend, &rpc, |_| panic!("漂移review不得进入模型")).is_err());
        rpc["review"] = review;
        assert!(
            bound_process_with(&backend, &rpc, |_| panic!("raw review不得作为旧RPC回退")).is_err()
        );
    }
}
