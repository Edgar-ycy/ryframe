//! 持同会话写入资格执行一次条件 PUT，并保留结果或未知状态的本地证据。
use std::{fs, path::Path};

use serde_json::{Value, json};

use super::{
    super::{
        copy_ledger::session::Session,
        digest, input, json as document,
        resources::common,
        s3::{self, Configuration, PutRequest},
        target_verify::{guard::Inputs, publication},
    },
    images::{self, TargetObject},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

pub(in crate::recovery::clone::native) struct Create<'a> {
    pub(crate) backend: &'a Path,
    pub(crate) configuration_root: &'a Path,
    pub(crate) artifact_root: &'a Path,
    pub(crate) work: &'a Path,
    pub(crate) directory: &'a Path,
    pub(crate) object: TargetObject<'a>,
    pub(crate) configuration: &'a Value,
    pub(crate) generation: &'a str,
    pub(crate) step: &'a str,
}

pub(in crate::recovery::clone::native) fn create(
    spec: &Create<'_>,
    session: &mut Session<'_>,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let (before, after) = images::object_pair(
        spec.generation,
        TargetObject {
            scope: spec.object.scope,
            endpoint: spec.object.endpoint,
            item: spec.object.item,
        },
    )?;
    let source = input::bound_file(spec.artifact_root, &spec.object.item["artifact"])?;
    session.write_once(
        spec.step,
        &before,
        &after,
        &source.digest.sha256,
        |authorize| {
            let check = || {
                checkpoint()?;
                authorize()
            };
            send_once(spec, &source, &check, s3::put_if_absent)
        },
    )
}

fn send_once(
    spec: &Create<'_>,
    source: &input::ArtifactBinding,
    check: &dyn Fn() -> Result<()>,
    send: impl FnOnce(PutRequest) -> Result<Value>,
) -> Result<()> {
    check()?;
    let work = common::local_directory(spec.backend, spec.work, "复制工作目录")?;
    let directory = validate_local_test_path(
        spec.directory,
        spec.backend,
        LocalTestPathKind::NewDirectory,
    )?;
    if directory.parent() != Some(work.as_path()) || spec.configuration["app_env"] != "test" {
        return Err("条件创建只接受已登记 test 配置及复制工作目录的新直接子目录".into());
    }
    let path = common::common_file(
        spec.backend,
        &spec.artifact_root.join(&source.file),
        "条件创建原对象载荷",
    )?;
    let held = Inputs::open([path.clone()])?;
    if digest::file(&path)? != source.digest {
        return Err("原对象载荷在保护句柄建立前变化".into());
    }
    let settings =
        s3::app_configuration::public_configuration(spec.configuration_root, spec.configuration)?;
    let binding = json!({"endpoint":settings["endpoint"],"region":settings["region"],"configuration":spec.configuration});
    if settings["endpoint"].as_str() != Some(spec.object.endpoint) {
        return Err("目标对象端点与已登记 TOML 不同".into());
    }
    let request = request(spec, source, &path, &binding)?;
    fs::create_dir(&directory)?;
    let intent = intent(spec, &request, &settings["region"]);
    let intent_path = spec.directory.join("intent.json");
    publication::publish(&intent_path, &intent)?;
    let frozen_intent = Inputs::open([intent_path])?;
    let diagnostic = spec.directory.join("diagnostic.json");
    let pending = evidence("needs_reconciliation", "write_result_not_recorded", true);
    publication::publish(&diagnostic, &pending)?;
    let pending_digest = digest::file(&diagnostic)?;

    let check_inputs = || {
        held.check()?;
        frozen_intent.check()?;
        Configuration::read_app_binding(spec.configuration_root, &binding)?;
        check()
    };
    let (outcome, execution) = match check_inputs() {
        Ok(()) => {
            let response = send(request);
            classify(response, check_inputs().is_ok())
        }
        Err(error) => (
            evidence("not_started", "preconditions_changed", false),
            Err(error),
        ),
    };
    let stored = finalize(&diagnostic, &pending_digest, &pending, &outcome);
    combine(execution, stored)
}

fn request(
    spec: &Create<'_>,
    source: &input::ArtifactBinding,
    path: &Path,
    binding: &Value,
) -> Result<PutRequest> {
    let item = spec.object.item;
    let bucket = document::text(&item["bucket"], "对象桶")?;
    let key = document::text(&item["target_key"], "目标对象 key")?;
    if !input::BUCKETS.contains(&bucket) {
        return Err("条件创建对象桶未登记".into());
    }
    s3::protocol::validate_key(spec.object.scope, key, false)?;
    Ok(PutRequest {
        configuration: Configuration::read_app_binding(spec.configuration_root, binding)?,
        bucket: bucket.to_owned(),
        key: key.to_owned(),
        source: path.to_path_buf(),
        expected_bytes: source.digest.bytes,
        expected_sha256: source.digest.sha256.clone(),
        metadata: item["metadata"].clone(),
    })
}

fn intent(spec: &Create<'_>, request: &PutRequest, region: &Value) -> Value {
    json!({"format_version":1,"kind":"ryframe-native-s3-put-if-absent-intent","transport":"rust-s3-sigv4",
        "scope_id":spec.object.scope,"endpoint":spec.object.endpoint,"region":region,
        "bucket":request.bucket,"key":request.key,"source":request.source,"step":spec.step,
        "generation_sha256":spec.generation,
        "expected":{"bytes":request.expected_bytes,"sha256":request.expected_sha256},"metadata":request.metadata})
}

fn classify(response: Result<Value>, unchanged: bool) -> (Value, Result<()>) {
    let (outcome, reason, may_write) = match response.as_ref() {
        _ if !unchanged => ("needs_reconciliation", "post_write_binding_changed", true),
        Err(_) => ("needs_reconciliation", "native_put_failed", true),
        Ok(value) => match (
            value["status"].as_str(),
            value["http_status"].as_u64(),
            value["remote_write_may_have_occurred"].as_bool(),
        ) {
            (Some("created_unverified"), Some(200), Some(true))
                if value["response"]["ETag"]
                    .as_str()
                    .is_some_and(|etag| !etag.is_empty()) =>
            {
                (
                    "created_unverified",
                    "independent_observation_required",
                    true,
                )
            }
            (Some("already_exists"), Some(412), Some(false)) => {
                ("already_exists", "precondition_rejected", false)
            }
            (Some("denied"), Some(401 | 403), Some(false)) => {
                ("denied", "permission_denied", false)
            }
            _ => ("needs_reconciliation", "native_put_unconfirmed", true),
        },
    };
    let mut record = evidence(outcome, reason, may_write);
    if let Ok(response) = response {
        record["http_status"] = response["http_status"]
            .as_u64()
            .filter(|status| (100..=599).contains(status))
            .map_or(Value::Null, |status| json!(status));
        if let Ok(sha) = digest::plan_hash(&response) {
            record["response_sha256"] = json!(sha);
        }
    }
    let result = if outcome == "created_unverified" {
        Ok(())
    } else {
        Err(format!("条件创建返回 {outcome}；保持原 intent，必须显式核对").into())
    };
    (record, result)
}

fn evidence(outcome: &str, reason: &str, may_write: bool) -> Value {
    json!({"format_version":1,"outcome":outcome,"reason":reason,
        "remote_write_may_have_occurred":may_write,"automatic_retry_permitted":false})
}

fn finalize(
    path: &Path,
    before: &digest::FileDigest,
    pending: &Value,
    result: &Value,
) -> Result<()> {
    if digest::file(path)? != *before || document::read(path)? != *pending {
        return Err("条件创建诊断已变化；保留原现场，不发布替代结果".into());
    }
    document::write_replace(path, result)?;
    if document::read(path)? != *result {
        return Err("条件创建诊断发布后变化；保留现场".into());
    }
    Ok(())
}

fn combine(primary: Result<()>, checked: Result<()>) -> Result<()> {
    match (primary, checked) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(other)) => Err(format!("{error}；条件创建诊断发布失败：{other}").into()),
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/object_write.rs"]
mod tests;
