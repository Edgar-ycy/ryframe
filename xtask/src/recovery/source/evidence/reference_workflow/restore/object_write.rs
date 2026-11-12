//! 受调用方检查点约束的单次条件 PUT，保留独立后像核验之前的证据。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::{
        clone::native::{
            digest, json as document, metadata,
            resources::common,
            s3::{self, Configuration, PutRequest, app_configuration},
            target_verify::{guard::Inputs, publication},
        },
        inputs::domain::BUCKETS,
    },
};

pub(super) struct Create<'a> {
    pub(super) backend: &'a Path,
    pub(super) work: &'a Path,
    pub(super) artifact_root: &'a Path,
    pub(super) task: &'a Value,
    pub(super) artifact: &'a Value,
    pub(super) target_s3: &'a Value,
    pub(super) scope: &'a str,
}

pub(super) fn send_once(
    spec: &Create<'_>,
    checkpoint: &dyn Fn() -> Result<()>,
    send: impl FnOnce(PutRequest) -> Result<Value>,
) -> Result<Value> {
    checkpoint().map_err(|_| "参考恢复对象条件创建前置检查未通过；未启动写请求")?;
    let work = common::local_directory(spec.backend, spec.work, "参考恢复对象证据目录")?;
    let request = prepare(spec)?;
    let mut snapshots = Vec::new();
    app_configuration::load(
        spec.backend,
        &spec.target_s3["configuration"],
        &mut snapshots,
    )?;
    let input_paths =
        std::iter::once(request.source.clone()).chain(snapshots.into_iter().map(|(path, _)| path));
    let held = Inputs::open(input_paths)?;
    let actual = digest::file(&request.source)?;
    if actual.bytes != request.expected_bytes || actual.sha256 != request.expected_sha256 {
        return Err("参考恢复原对象载荷在建立保护期间变化".into());
    }

    let intent = intent(spec, &request);
    let attempt = Attempt::new(&work, &intent)?;
    let Ok(evidence) = Inputs::open([attempt.request.clone(), attempt.diagnostic.clone()]) else {
        return attempt.finish(Outcome::NotStarted);
    };
    let check_inputs = || -> Result<()> {
        checkpoint()?;
        held.check()?;
        evidence.check()?;
        Configuration::read_app_binding(spec.backend, spec.target_s3)?;
        Ok(())
    };
    let outcome = if check_inputs().is_err() {
        Outcome::NotStarted
    } else {
        let response = send(request);
        classify(response, check_inputs().is_ok())
    };
    drop(evidence);
    attempt.finish(outcome)
}

fn prepare(spec: &Create<'_>) -> Result<PutRequest> {
    document::exact(
        spec.task,
        &["kind", "bucket", "key", "relative", "metadata"],
        "参考恢复对象任务",
    )?;
    if spec.task["kind"] != "object" {
        return Err("参考恢复条件创建只接受对象任务".into());
    }
    common::scope_id(spec.scope)?;
    let bucket = document::text(&spec.task["bucket"], "参考恢复对象桶")?;
    let key = document::text(&spec.task["key"], "参考恢复对象键")?;
    if !BUCKETS.contains(&bucket) {
        return Err("参考恢复对象桶未登记".into());
    }
    s3::protocol::validate_key(spec.scope, key, false)?;
    let root = common::local_directory(spec.backend, spec.artifact_root, "原备份根目录")?;
    let relative = document::text(&spec.task["relative"], "原备份对象相对路径")?;
    let source = digest::safe_file(&root, relative)?;
    let source = common::common_file(spec.backend, &source, "原备份对象载荷")?;
    let artifact = document::exact(
        spec.artifact,
        &["path", "bytes", "sha256"],
        "原备份对象绑定",
    )?;
    if source != Path::new(document::text(&artifact["path"], "原备份对象路径")?) {
        return Err("参考恢复对象任务与原备份文件路径不同".into());
    }
    let expected_bytes = artifact["bytes"].as_u64().ok_or("原对象字节数无效")?;
    let expected_sha256 = document::text(&artifact["sha256"], "原对象摘要")?.to_owned();
    Ok(PutRequest {
        configuration: Configuration::read_app_binding(spec.backend, spec.target_s3)?,
        bucket: bucket.to_owned(),
        key: key.to_owned(),
        source,
        expected_bytes,
        expected_sha256,
        metadata: metadata::copy(&spec.task["metadata"])?,
    })
}

fn intent(spec: &Create<'_>, request: &PutRequest) -> Value {
    json!({"format_version":1,"kind":"ryframe-native-s3-put-if-absent-intent","transport":"rust-s3-sigv4",
        "scope_id":spec.scope,"endpoint":spec.target_s3["endpoint"],"region":spec.target_s3["region"],
        "bucket":request.bucket,"key":request.key,"source":request.source,
        "expected":{"bytes":request.expected_bytes,"sha256":request.expected_sha256},"metadata":request.metadata})
}

struct Attempt {
    request: PathBuf,
    diagnostic: PathBuf,
    pending: Value,
    pending_digest: digest::FileDigest,
}

impl Attempt {
    fn new(work: &Path, intent: &Value) -> Result<Self> {
        let request = work.join(format!(
            "object-create-{}.json",
            uuid::Uuid::new_v4().simple()
        ));
        let diagnostic = request.with_extension("diagnostic.json");
        publication::publish(&request, intent)?;
        let pending = Outcome::Unknown(Some("WriteResultNotRecorded")).diagnostic(&request);
        publication::publish(&diagnostic, &pending)?;
        let pending_digest = digest::file(&diagnostic)?;
        Ok(Self {
            request,
            diagnostic,
            pending,
            pending_digest,
        })
    }

    fn finish(self, outcome: Outcome) -> Result<Value> {
        let record = outcome.diagnostic(&self.request);
        if digest::file(&self.diagnostic)? != self.pending_digest
            || document::read(&self.diagnostic)? != self.pending
        {
            return Err("对象条件创建诊断变化；保留原现场，禁止自动重试".into());
        }
        document::write_replace(&self.diagnostic, &record)?;
        if document::read(&self.diagnostic)? != record {
            return Err("对象条件创建诊断在发布后变化；保留现场".into());
        }
        if let Outcome::Created(response) = outcome {
            return Ok(json!({"status":"created_unverified","response":response,
                "request_file":self.request,"diagnostic_file":self.diagnostic}));
        }
        Err(format!(
            "对象条件创建未确认：{}；检查 {}；禁止降级或自动重试",
            outcome.fields().0,
            self.diagnostic.display()
        )
        .into())
    }
}

enum Outcome {
    NotStarted,
    AlreadyExists,
    Denied,
    Created(Value),
    Unknown(Option<&'static str>),
}

impl Outcome {
    fn fields(&self) -> (&'static str, Option<&'static str>, bool) {
        match self {
            Self::NotStarted => ("not_started", Some("PreconditionsChanged"), false),
            Self::AlreadyExists => ("already_exists", None, false),
            Self::Denied => ("denied", None, false),
            Self::Created(_) => ("created_unverified", None, true),
            Self::Unknown(error) => ("needs_reconciliation", *error, true),
        }
    }

    fn diagnostic(&self, request: &Path) -> Value {
        let (status, error_type, may_write) = self.fields();
        // 进程内调用无子进程退出码，字段保留为空。
        json!({"format_version":1,"request_file":request,"outcome":status,"returncode":null,
            "error_type":error_type,"remote_write_may_have_occurred":may_write,"automatic_retry_permitted":false})
    }
}

fn classify(result: Result<Value>, unchanged: bool) -> Outcome {
    if !unchanged {
        return Outcome::Unknown(Some("PostWriteBindingChanged"));
    }
    let Ok(value) = result else {
        return Outcome::Unknown(Some("NativePutFailed"));
    };
    if value["automatic_retry_permitted"] != false
        || value.as_object().is_none_or(|fields| {
            fields.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "status"
                        | "reason"
                        | "http_status"
                        | "response"
                        | "remote_write_may_have_occurred"
                        | "automatic_retry_permitted"
                )
            })
        })
    {
        return Outcome::Unknown(Some("InvalidOutcome"));
    }
    match (
        value["status"].as_str(),
        value["http_status"].as_u64(),
        value["remote_write_may_have_occurred"].as_bool(),
    ) {
        (Some("created_unverified"), Some(200), Some(true)) => {
            match response_identity(&value["response"]) {
                Some(response) => Outcome::Created(response),
                None => Outcome::Unknown(Some("InvalidOutcome")),
            }
        }
        (Some("already_exists"), Some(412), Some(false)) => Outcome::AlreadyExists,
        (Some("denied"), Some(401 | 403), Some(false)) => Outcome::Denied,
        (Some("needs_reconciliation"), _, Some(true)) => Outcome::Unknown(None),
        _ => Outcome::Unknown(Some("InvalidOutcome")),
    }
}

fn response_identity(value: &Value) -> Option<Value> {
    let fields = value.as_object()?;
    let text = |key| {
        fields
            .get(key)?
            .as_str()
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
    };
    let etag = text("ETag")?;
    match fields.len() {
        1 => Some(json!({"ETag":etag})),
        2 => Some(json!({"ETag":etag,"VersionId":text("VersionId")?})),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/source/evidence/reference_workflow/restore/object_write_tests.rs"]
mod tests;
