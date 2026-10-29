//! reset 聚合绑定原计划、精确命令与最终 release 的直接前像。
use super::super::{input::digest_shape, json as json_io};
use super::{evidence_model::*, input::request_physical, reset};
use crate::Result;
use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) const MANIFEST_VERSION_ERROR: &str = "reset 清单 manifest_version 必须为 5；维护构建与协调工具的协议版本不匹配。请核对二者来源及版本，保留现存证据并先检查 status，不自动重放初始化";

pub(crate) fn aggregate(
    backend: &Path,
    target: &Path,
    request: &Value,
    original: &Value,
) -> Result<Value> {
    let configuration = super::fresh::initialize::reset_configuration::Snapshot::open(
        backend,
        target,
        request,
        &original["maintenance"],
    )?;
    let reset_request = configuration.request(request)?;
    let result = assemble(
        backend,
        target,
        request,
        original,
        &reset_request,
        Some(configuration.binding()?),
    );
    configuration.check()?;
    result
}

// 仅已完成终态的历史读取可识别旧合同；初始化、写入及续作始终调用严格 aggregate。
pub(crate) fn history(
    backend: &Path,
    target: &Path,
    request: &Value,
    original: &Value,
) -> Result<Value> {
    let terminal = read(backend, &target.join("initialized.json"))?;
    let legacy = terminal["status"] == "fresh_target_initialized"
        && terminal["reset"]
            .as_object()
            .is_some_and(|value| !value.contains_key("configuration"))
        && terminal["history"]
            .as_object()
            .is_some_and(|value| !value.contains_key("reset-configuration.json"));
    if !legacy {
        return aggregate(backend, target, request, original);
    }
    json_io::exact(
        &terminal,
        super::resume_model::INITIALIZED,
        "旧 reset 完整终态",
    )?;
    let locked = target.join("initialize.lock").try_exists()?;
    let tree = super::published::tree::initialized(backend, target, locked)?;
    check(
        !target.join("reset-configuration.json").try_exists()?
            && terminal["generation"] == *original
            && terminal["id"] == request["id"]
            && terminal["scope_id"] == request["target"]["scope_id"]
            && terminal["history"] == super::proof::history(backend, target)?
            && read(backend, &target.join("initialized-candidate.json"))? == terminal,
        "旧 reset 合同只接受完整已完成终态；混合或删除新快照绑定均拒绝",
    )?;
    let environment = BTreeMap::from([("APP_ENV".into(), "test".into())]);
    let inputs = super::storage::StorageInputs {
        backend,
        request,
        environment: &environment,
        observation_dir: target,
    };
    let root = inputs.execution_backend()?;
    let (directory, files) =
        super::input::request_configuration::fresh_files(&root, &environment, request)?;
    let protected = super::guard::Inputs::open(files.keys().map(|name| directory.join(name)))?;
    check(
        super::input::request_configuration::configuration_identity(&directory, &files)?
            == json_io::text(&request["configuration_sha256"], "原 E 配置摘要")?
            && original["configuration_sha256"] == request["configuration_sha256"],
        "旧 reset 原 E 绑定变化",
    )?;
    let table = crate::rate_limit::load_app_table(&root, Some(&directory), "test")
        .map_err(|_| "旧 reset 原 E 不能安全读取")?;
    super::creation_plan::reset_configuration(
        target,
        &serde_json::to_value(table)?,
        &original["maintenance"],
    )?;
    let result = assemble(backend, target, request, original, request, None)?;
    check(
        result["completed"] == terminal["reset"],
        "旧 reset 完成聚合与终态不同",
    )?;
    check(
        super::input::request_configuration::fresh_files(&root, &environment, request)?
            == (directory, files),
        "旧 reset 配置集合在读取期间变化",
    )?;
    protected.check()?;
    check(
        super::published::tree::initialized(backend, target, locked)? == tree,
        "旧 reset 完整历史树在读取期间变化",
    )?;
    Ok(result)
}

pub(crate) fn history_request(
    backend: &Path,
    target: &Path,
    request: &Value,
    original: &Value,
) -> Result<Value> {
    if history(backend, target, request, original)?
        .get("configuration")
        .is_none()
    {
        return Ok(request.clone());
    }
    super::fresh::initialize::reset_configuration::Snapshot::open(
        backend,
        target,
        request,
        &original["maintenance"],
    )?
    .request(request)
}

fn assemble(
    backend: &Path,
    target: &Path,
    request: &Value,
    original: &Value,
    reset_request: &Value,
    configuration: Option<Value>,
) -> Result<Value> {
    let mut actual = names(target, r"^reset-plan.*\.json$")?;
    actual.retain(|name| !name.ends_with(".command.json"));
    actual.extend(names(target, r"^reset.*\.(intent|confirmed)\.json$")?);
    check(
        actual
            == [
                "reset-plan.json",
                "reset-plan-confirm.json",
                "reset.intent.json",
                "reset.confirmed.json",
            ]
            .map(str::to_owned)
            .into(),
        "reset 阶段包含缺失、重复命名或未知证据",
    )?;
    let first = read(backend, &target.join("reset-plan.json"))?;
    let second = read(backend, &target.join("reset-plan-confirm.json"))?;
    for plan in [&first, &second] {
        json_io::exact(plan, &["manifest", "plan_hash"], "固定 reset 计划")?;
    }
    check(first == second, "原 reset 两次固定计划证据不一致")?;
    validate_manifest(backend, &first["manifest"], request, original)?;
    let mut completed = reset::completed(
        backend,
        target,
        &first["manifest"],
        json_io::text(&first["plan_hash"], "reset hash")?,
    )?;
    if let Some(binding) = &configuration {
        completed["configuration"] = binding.clone();
    }
    let intent = stage_record(
        backend,
        target,
        "reset",
        "intent",
        &json!({"plan_hash":first["plan_hash"]}),
    )?;
    let confirmed = stage_record(backend, target, "reset", "confirmed", &completed)?;
    let commands = commands(
        backend,
        target,
        &original["maintenance"],
        &first,
        &second,
        &completed,
        reset_request,
    )?;
    let mut result = json!({"plan":binding(backend,&target.join("reset-plan.json"))?,
        "plan_confirm":binding(backend,&target.join("reset-plan-confirm.json"))?,
        "intent":intent,"confirmed":confirmed,"commands":commands,"completed":completed});
    if let Some(binding) = configuration {
        result["configuration"] = binding;
    }
    Ok(result)
}

fn commands(
    backend: &Path,
    target: &Path,
    maintenance: &Value,
    first: &Value,
    second: &Value,
    completed: &Value,
    request: &Value,
) -> Result<Value> {
    let all = names(target, r"^reset-.*\.command\.json$")?;
    let executable = executable(maintenance, "reset")?;
    let mut found = Map::new();
    let mut consumed = BTreeSet::new();
    for stage in ["reset-plan", "reset-plan-confirm", "reset-execute"] {
        let pattern = Regex::new(&format!(
            r"^{}-[a-f0-9]{{32}}\.command\.json$",
            regex::escape(stage)
        ))?;
        let paths = all
            .iter()
            .filter(|name| pattern.is_match(name))
            .collect::<Vec<_>>();
        check(
            paths.len() == 1,
            "reset 必须为 plan、复核和 execute 各保留唯一命令收据",
        )?;
        let name = paths[0];
        let path = target.join(name);
        let descriptor = if stage != "reset-execute" {
            reset::plan_receipt(
                backend,
                target,
                &path,
                executable,
                if stage == "reset-plan" { first } else { second },
                request,
            )?
        } else {
            let value = read(backend, &path)?;
            json_io::exact(
                &value,
                &["command", "returncode", "error_type", "stdout", "stderr"],
                "reset 执行收据",
            )?;
            let report = target.join(json_io::text(&completed["report"]["file"], "reset 报告")?);
            let stdout = format!(
                "reset_status=completed\nreset_completed_at={}\nreset_report={}\n",
                json_io::text(&completed["completed_at"], "reset 完成时间")?,
                report.display()
            );
            let wanted = json!(super::resume_prefix::configured_command(
                executable,
                request,
                vec![
                    "execute".into(),
                    "--plan-hash".into(),
                    json_io::text(&first["plan_hash"], "reset hash")?.into(),
                    "--confirm-reset".into(),
                    json_io::text(&first["manifest"]["confirmation_phrase"], "reset确认")?.into()
                ]
            )?);
            check(
                value["returncode"].as_i64() == Some(0)
                    && value["error_type"].is_null()
                    && value["stderr"] == ""
                    && value["command"] == wanted
                    && value["stdout"]
                        .as_str()
                        .map(|text| text.replace("\r\n", "\n"))
                        == Some(stdout),
                "reset execute 收据与固定计划或完成报告不一致",
            )?;
            binding(backend, &path)?
        };
        found.insert(stage.to_owned(), descriptor);
        consumed.insert(name.clone());
    }
    check(consumed == all, "reset 阶段包含未知命令收据")?;
    Ok(Value::Object(found))
}

pub(crate) fn validate_manifest(
    backend: &Path,
    manifest: &Value,
    request: &Value,
    original: &Value,
) -> Result<()> {
    json_io::exact(
        manifest,
        &[
            "manifest_version",
            "environment",
            "scope_id",
            "code_sha",
            "config_sha",
            "credential_version",
            "confirmation_phrase",
            "legacy_ownership",
            "redis",
            "object_storage",
            "databases",
        ],
        "reset 清单",
    )?;
    let target = &request["target"];
    let scope = json_io::text(&target["scope_id"], "reset scope")?;
    check(manifest["manifest_version"] == 5, MANIFEST_VERSION_ERROR)?;
    check(
        manifest["environment"] == "test"
            && manifest["scope_id"] == target["scope_id"]
            && manifest["code_sha"] == original["maintenance"]["source"]["snapshot"]["head"]
            && manifest["credential_version"] == request["reset"]["credential_version"]
            && manifest["confirmation_phrase"] == format!("RESET-RYFRAME-test-{scope}")
            && manifest["legacy_ownership"] == request["reset"]["legacy_ownership"],
        "实际 reset plan 来源或引导范围不符",
    )?;
    digest_shape(&manifest["config_sha"])?;
    databases(backend, manifest, request, original, scope)?;
    objects(manifest, &original["physical"]["s3"], scope)?;
    redis(manifest, request, original)
}

fn redis(manifest: &Value, request: &Value, original: &Value) -> Result<()> {
    let redis = &manifest["redis"];
    json_io::exact(
        redis,
        &[
            "host",
            "port",
            "database",
            "namespace",
            "ownership_marker_key",
            "ownership_marker",
            "outside_sentinel_key_sha256",
            "password_config_key",
            "tls",
            "tls_ca_sha256",
            "tls_client_cert_sha256",
            "tls_client_key_ref_sha256",
        ],
        "reset Redis",
    )?;
    let selected = &original["selected"]["redis"];
    let sentinel = json_io::text(&request["reset"]["sentinel_key"], "reset sentinel")?;
    check(
        redis["host"] == "127.0.0.1"
            && redis["port"] == request["storage"]["redis"]["port"]
            && redis["database"] == 0
            && redis["tls"] == false
            && redis["password_config_key"] == "redis.password"
            && [
                "tls_ca_sha256",
                "tls_client_cert_sha256",
                "tls_client_key_ref_sha256",
            ]
            .iter()
            .all(|field| redis[*field].is_null())
            && redis["namespace"] == selected["namespace"]
            && redis["ownership_marker_key"] == selected["ownership_key"]
            && redis["ownership_marker"] == selected["ownership_value"]
            && redis["outside_sentinel_key_sha256"]
                == hex::encode(Sha256::digest(sentinel.as_bytes())),
        "reset plan Redis namespace、端点或 sentinel 不符",
    )
}

fn databases(
    backend: &Path,
    manifest: &Value,
    request: &Value,
    original: &Value,
    scope: &str,
) -> Result<()> {
    let physical = json_io::array(&original["physical"]["databases"], "物理数据库")?
        .iter()
        .map(|item| Ok((json_io::text(&item["key"], "物理 key")?, item)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let items = json_io::array(&manifest["databases"], "reset 数据库")?;
    check(items.len() == 4, "reset plan 包含额外或缺失数据库")?;
    let mut seen = BTreeSet::new();
    for item in items {
        json_io::exact(
            item,
            &[
                "host",
                "port",
                "database",
                "connection",
                "target_keys",
                "control_baseline",
                "tenant_baseline",
                "ownership_markers",
            ],
            "reset 数据库",
        )?;
        let keys = json_io::array(&item["target_keys"], "reset 目标键")?;
        check(keys.len() == 1, "reset plan 数据库合并或重复")?;
        let key = json_io::text(&keys[0], "reset 目标 key")?;
        check(
            super::input::request::KEYS
                .iter()
                .any(|entry| entry.0 == key)
                && seen.insert(key),
            "reset plan 数据库合并或重复",
        )?;
        let expected = physical.get(key).ok_or("reset 物理目标缺失")?;
        let source = json_io::array(&request["target"]["databases"], "请求数据库")?
            .iter()
            .find(|db| db["key"] == key)
            .ok_or("reset 请求目标缺失")?;
        let connection = request_physical::defaults_connection(backend, source)?;
        database_connection(&item["connection"], expected, &connection, key)?;
        let mut owners = Map::new();
        for kind in if key == "shared-control" {
            &["control", "tenant-data"][..]
        } else {
            &["tenant-data"][..]
        } {
            owners.insert(
                (*kind).to_owned(),
                json!(format!("ryframe-owner:v1:{scope}:{kind}")),
            );
        }
        check(
            ["host", "port", "database"]
                .iter()
                .all(|field| item[*field] == expected[*field])
                && item["control_baseline"] == json!(key == "shared-control")
                && item["tenant_baseline"] == true
                && item["ownership_markers"] == Value::Object(owners),
            "reset plan 数据库实际目标、baseline 或 owner 不符",
        )?;
    }
    Ok(())
}

fn database_connection(
    actual: &Value,
    expected: &Value,
    connection: &Value,
    key: &str,
) -> Result<()> {
    json_io::exact(
        actual,
        &[
            "username",
            "password_config_key",
            "tls_mode",
            "tls_ca_sha256",
            "tls_client_cert_sha256",
            "tls_client_key_ref_sha256",
        ],
        "reset 数据库连接",
    )?;
    let password_reference = if key == "shared-control" {
        "database.primary.password".to_owned()
    } else {
        format!("tenant_data.targets.{key}.password")
    };
    check(
        actual["tls_mode"] == expected["tls_mode"]
            && actual["username"] == connection["username"]
            && actual["password_config_key"] == password_reference
            && [
                "tls_ca_sha256",
                "tls_client_cert_sha256",
                "tls_client_key_ref_sha256",
            ]
            .iter()
            .all(|field| actual[*field].is_null()),
        "reset plan 数据库连接、凭据字段引用或 TLS 不符",
    )
}

fn objects(manifest: &Value, expected: &Value, scope: &str) -> Result<()> {
    let storage = &manifest["object_storage"];
    json_io::exact(
        storage,
        &[
            "backend",
            "endpoint",
            "use_ssl",
            "region",
            "access_key_config_key",
            "secret_key_config_key",
            "prefixes",
        ],
        "reset 对象存储",
    )?;
    let prefixes = json_io::array(&storage["prefixes"], "reset 对象前缀")?;
    let buckets = prefixes
        .iter()
        .map(|item| json_io::text(&item["bucket"], "对象桶"))
        .collect::<Result<BTreeSet<_>>>()?;
    check(
        ["backend", "endpoint", "region"]
            .iter()
            .all(|field| storage[*field] == expected[*field])
            && storage["access_key_config_key"] == "object_storage.access_key"
            && storage["secret_key_config_key"] == "object_storage.secret_key"
            && storage["use_ssl"]
                == json!(json_io::text(&expected["endpoint"], "对象端点")?.starts_with("https://"))
            && prefixes.len() == 5
            && buckets == BUCKETS.into(),
        "reset plan 对象端点或五桶不符",
    )?;
    for item in prefixes {
        let bucket = json_io::text(&item["bucket"], "对象桶")?;
        check(
            *item
                == json!({"bucket":bucket,"prefix":format!("{scope}/"),"ownership_marker_key":format!("{scope}/.ryframe-owner"),
            "ownership_marker":format!("ryframe-owner:v1:{scope}:object-storage:{bucket}")}),
            "reset plan 对象前缀越界",
        )?;
    }
    Ok(())
}
