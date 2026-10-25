//! 每次尝试的固定观察序列；完整组精确匹配，未完成组只能是有绑定的合法中断前缀。
use super::{
    super::{digest, json as document, resources::common},
    evidence_model::check,
    fresh::selection::Selected,
    resume_observations, storage,
};
use crate::Result;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

struct Slot {
    name: String,
    expected: Value,
}

pub(super) fn validate(
    backend: &Path,
    target: &Path,
    names: &BTreeSet<String>,
    request: &Value,
    candidate: &Selected,
    complete: bool,
    progress: &mut usize,
) -> Result<()> {
    let slots = slots(backend, request, candidate)?;
    let mut remaining = names.clone();
    let mut done = Vec::new();
    for slot in &slots {
        let matches = remaining
            .iter()
            .filter(|name| category(name) == slot.name)
            .cloned()
            .collect::<Vec<_>>();
        let mut successful = None;
        let mut failed = None;
        for name in matches {
            let value = document::read(&target.join(&name))?;
            if matches_slot(&value, &slot.expected, true).is_ok() {
                successful = Some(name);
                break;
            }
            if matches_slot(&value, &slot.expected, false).is_ok() {
                failed = Some(name);
            }
        }
        if let Some(name) = successful {
            remaining.remove(&name);
            done.push(name);
            *progress += 1;
        } else {
            if let Some(name) = failed {
                remaining.remove(&name);
            }
            check(!complete && remaining.is_empty(), missing(&slot.name))?;
            return Ok(());
        }
    }
    check(remaining.is_empty(), "prepare 观察组包含未消耗增量")?;
    // 两轮内核观察只比较稳定身份，调度态和累计计数不属于身份。
    let redis = done
        .into_iter()
        .filter(|name| category(name) == "redis-kernel")
        .collect::<BTreeSet<_>>();
    resume_observations::redis(backend, target, &redis, &redis, 2)
}

fn category(name: &str) -> String {
    name.rsplit_once('-')
        .map(|(prefix, _)| prefix.to_owned())
        .unwrap_or_default()
}

fn slots(backend: &Path, request: &Value, candidate: &Selected) -> Result<Vec<Slot>> {
    let mut descriptor = document::object(&request["review"], "审阅绑定")?.clone();
    descriptor.remove("canonical_sha256");
    let review = common::read_bound(backend, &Value::Object(descriptor), "prepare 审阅")?;
    let layout = storage::rustfs::evidence(
        backend,
        &review,
        &candidate.value["rustfs"],
        candidate.runtime.as_ref(),
    )?;
    let kernel = kernel_slots(&candidate.value["redis"])?;
    let mut result = vec![Slot {
        name: "storage-layout".into(),
        expected: layout.clone(),
    }];
    result.extend(kernel_slots(&candidate.value["redis"])?);
    for db in document::array(&request["target"]["databases"], "prepare 四库")? {
        let command = json!([
            request["tools"]["mysql"]["path"],
            format!(
                "--defaults-file={}",
                document::text(&db["defaults_file"], "MySQL 文件")?
            ),
            "--connect-timeout=5",
            "--batch",
            "--raw",
            "--skip-column-names",
            "--binary-mode",
            "--default-character-set=utf8mb4"
        ]);
        result.push(Slot {name:format!("mysql-{}",document::text(&db["key"],"数据库 key")?),
            expected:json!({"command":command,"stdout":format!("{}\n",document::text(&db["server_uuid"],"MySQL UUID")?)})});
    }
    for bucket in super::super::input::BUCKETS
        .into_iter()
        .collect::<BTreeSet<_>>()
    {
        for (stage, operation) in [
            ("bucket-readiness", "head-bucket"),
            ("bucket-versioning", "get-bucket-versioning"),
            ("scoped-list", "list-objects-v2"),
        ] {
            result.push(Slot {name:stage.into(),expected:json!({"operation":operation,"bucket":bucket,
                "scope_id":request["target"]["scope_id"],"endpoint":request["target"]["s3"]["endpoint"],
                "configuration_sha256":digest::plan_hash(&request["target"]["s3"])?,
                "prefix":if stage == "scoped-list" {json!(format!("{}/",document::text(&request["target"]["scope_id"],"对象 scope")?))} else {Value::Null}})});
        }
    }
    result.push(Slot {
        name: "storage-layout".into(),
        expected: layout,
    });
    result.extend(kernel);
    Ok(result)
}

fn kernel_slots(redis: &Value) -> Result<Vec<Slot>> {
    let config = recorded_configuration(Path::new(document::text(
        &redis["configuration"]["path"],
        "Redis 配置",
    )?))?;
    let pid = redis["pid"].as_u64().ok_or("Redis PID 无效")?;
    let exe = document::text(&redis["executable"], "Redis 映像")?;
    let commands = [
        (
            vec!["/usr/bin/cat".into(), format!("/proc/{pid}/stat")],
            json!({"pid":pid,"started":redis["started"]}),
        ),
        (
            vec![
                "/usr/bin/readlink".into(),
                "-f".into(),
                format!("/proc/{pid}/exe"),
            ],
            json!({"text":exe}),
        ),
        (
            vec!["/usr/bin/sha256sum".into(), exe.into()],
            json!({"sha":redis["sha256"],"file":exe}),
        ),
        (
            vec!["/usr/bin/readlink".into(), "-f".into(), config.clone()],
            json!({"text":config}),
        ),
        (
            vec!["/usr/bin/sha256sum".into(), config.clone()],
            json!({"sha":redis["configuration"]["sha256"],"file":config}),
        ),
    ];
    let mut result = Vec::new();
    for (parts, output) in commands {
        let mut command = vec![
            redis["wsl"]["path"].clone(),
            json!("--distribution"),
            redis["distribution"].clone(),
            json!("--exec"),
        ];
        command.extend(parts.into_iter().map(Value::String));
        result.push(Slot {
            name: "redis-kernel".into(),
            expected: json!({"command":command,"kernel":output}),
        });
    }
    Ok(result)
}

fn matches_slot(value: &Value, expected: &Value, success: bool) -> Result<()> {
    if expected.get("process_receipt").is_some() {
        return check(
            success && value == expected,
            "RustFS 观察不同于唯一账本代次",
        );
    }
    if expected.get("operation").is_some() {
        return object(value, expected, success);
    }
    document::exact(
        value,
        &["command", "returncode", "error_type", "stdout", "stderr"],
        "prepare 固定只读收据",
    )?;
    check(
        value["command"] == expected["command"]
            && value["stdout"].is_string()
            && value["stderr"].is_string(),
        "prepare 观察命令不是固定只读目标",
    )?;
    if !success {
        return check(
            ((value["returncode"].is_null()
                || value["returncode"].as_i64().is_some_and(|v| v != 0))
                && value["error_type"].is_string())
                || (value["returncode"] == 0
                    && matches!(
                        value["error_type"].as_str(),
                        Some("InvalidUtf8" | "UnexpectedStderr")
                    )
                    && value["stdout"] == ""),
            "prepare 中断收据没有明确失败",
        );
    }
    check(
        value["returncode"] == 0 && value["error_type"].is_null() && value["stderr"] == "",
        "prepare 只读命令没有成功",
    )?;
    let stdout = document::text(&value["stdout"], "prepare 输出")?.replace("\r\n", "\n");
    if expected.get("kernel").is_none() {
        return check(expected["stdout"] == stdout, "MySQL 空前像不属于固定 UUID");
    }
    let kernel = &expected["kernel"];
    if kernel.get("pid").is_some() {
        let (head, started) = resume_observations::process_identity(&stdout)?;
        check(
            head.starts_with(&format!("{} (", kernel["pid"])) && kernel["started"] == started,
            "Redis PID 或创建代次不同",
        )
    } else if let Some(text) = kernel.get("text") {
        check(
            stdout == format!("{}\n", document::text(text, "Redis 固定路径")?),
            "Redis 固定路径不同",
        )
    } else {
        let raw = format!(
            "{}  {}\n",
            document::text(&kernel["sha"], "固定 SHA")?,
            document::text(&kernel["file"], "固定文件")?
        );
        check(stdout == raw, "Redis 固定文件摘要或路径不同")
    }
}

fn object(value: &Value, expected: &Value, success: bool) -> Result<()> {
    document::exact(
        value,
        &[
            "transport",
            "operation",
            "bucket",
            "returncode",
            "error_type",
            "response",
            "key",
            "destination",
            "remote_writes",
            "scope_id",
            "endpoint",
            "configuration_sha256",
            "prefix",
            "continuation",
        ],
        "prepare 对象只读收据",
    )?;
    for (key, field) in document::object(expected, "固定对象目标")? {
        check(
            value.get(key) == Some(field),
            "prepare 对象目标、配置或范围不同",
        )?;
    }
    check(
        value["transport"] == "rust-s3-sigv4"
            && value["remote_writes"] == 0
            && ["key", "destination", "continuation"]
                .iter()
                .all(|k| value[*k].is_null()),
        "prepare 对象观察越界",
    )?;
    if !success {
        return check(
            value["error_type"].is_string()
                && (value["returncode"].is_null()
                    || value["returncode"].as_i64().is_some_and(|v| v != 0)),
            "对象中断没有明确失败",
        );
    }
    check(
        value["returncode"] == 0 && value["error_type"].is_null() && value["response"].is_object(),
        "对象观察未成功",
    )?;
    match expected["operation"].as_str() {
        Some("get-bucket-versioning") => {
            check(value["response"]["Status"].is_null(), "对象桶已启用版本")
        }
        Some("list-objects-v2") => check(
            value["response"]["IsTruncated"] == false
                && value["response"]
                    .get("Contents")
                    .is_none_or(|v| v == &json!([])),
            "对象范围不是完整空像",
        ),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/target_verify/prepare_history.rs"]
mod tests;

/// 历史模型保留文件所属平台的绝对路径；Windows 文件必须使用精确 WSL 挂载映射。
/// 这里不观察命名空间或端口，实际 Windows 资源端口仍使用 redis_layout::wsl_path。
pub(super) fn recorded_configuration(path: &Path) -> Result<String> {
    let text = path.to_str().ok_or("Redis 配置路径不是 UTF-8")?;
    if text.starts_with('/') && path.is_absolute() {
        check(
            !text.split('/').any(|part| matches!(part, "." | "..")),
            "Redis 历史配置路径不规范",
        )?;
        return Ok(text.to_owned());
    }
    storage::redis_layout::wsl_path(path)
}

fn missing(category: &str) -> &'static str {
    if category.starts_with("mysql-") {
        "fresh prepare MySQL 缺失观察不属于固定目标"
    } else if category == "redis-kernel" {
        "fresh prepare Redis /proc stat 观察不属于固定进程"
    } else if category == "storage-layout" {
        "fresh prepare 存储目录两轮观察不一致"
    } else {
        "fresh prepare 对象前缀并非明确空范围"
    }
}
