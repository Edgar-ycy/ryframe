//! 观察已绑定的单个目标对象，保留原始证据，不执行对象写入。
use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    super::{
        digest::{self, FileDigest},
        export::transport::Transport,
        input::BUCKETS,
        json as document,
        lifecycle::manifest,
        resources::common,
        s3::{Configuration, Operation, Request, app_configuration, protocol::validate_key},
        target_verify::guard::Inputs,
    },
    images::ObjectObservation,
};
use crate::{
    Result,
    local_test_path::strip_local_test_prefix,
    recovery::{
        inputs::document as binding,
        source::evidence::{inspect_object, object_capture_result},
    },
};

pub(crate) struct ObserveTarget<'a> {
    pub(crate) backend: &'a Path,
    pub(crate) configuration_root: &'a Path,
    pub(crate) work: &'a Path,
    pub(crate) directory: &'a Path,
    pub(crate) plan_sha256: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) endpoint: &'a str,
    pub(crate) region: &'a str,
    pub(crate) configuration: &'a Value,
    pub(crate) bucket: &'a str,
    pub(crate) key: &'a str,
    pub(crate) expected: &'a Value,
    pub(crate) max_bytes: u64,
}

pub(crate) struct Observed {
    receipt: Value,
    descriptor: Value,
    raw_sha256: String,
    capture: Option<Value>,
}

impl Observed {
    pub(crate) fn image_evidence(&self) -> ObjectObservation<'_> {
        match &self.capture {
            Some(capture) => ObjectObservation::Captured(capture),
            None => ObjectObservation::Absent {
                receipt: &self.receipt,
                descriptor: &self.descriptor,
                raw_sha256: &self.raw_sha256,
            },
        }
    }

    pub(crate) fn receipt(&self) -> &Value {
        &self.receipt
    }

    pub(crate) fn descriptor(&self) -> &Value {
        &self.descriptor
    }

    pub(crate) fn check_failure(&self) -> Result<()> {
        let saved = binding::binding_shape(&self.descriptor, "目标对象观察收据")?;
        reject_failure(saved.path.parent().ok_or("目标对象观察收据缺少目录")?)
    }
}

pub(crate) fn reject_failure(directory: &Path) -> Result<()> {
    match fs::symlink_metadata(directory.join("failure.json")) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("对象证据目录含失败标记".into()),
    }
}

pub(crate) fn observe<T: Transport>(
    request: ObserveTarget<'_>,
    transport: &T,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<Observed> {
    let capture_intent = request.intent()?;
    let input_binding = json!({
        "plan_sha256": request.plan_sha256,
        "configuration_sha256": digest::plan_hash(&json!({"configuration":request.configuration}))?,
        "transport":"rust-s3-sigv4"
    });
    let evidence = Evidence::new(request.backend, request.work, request.directory)?;
    let mut observation = Observation {
        request,
        transport,
        checkpoint,
        evidence,
        configuration_inputs: None,
        stage: "binding",
    };
    match observation.run(&capture_intent, &input_binding) {
        Ok(observed) => Ok(observed),
        Err(_) => {
            let retained = observation.evidence.write(
                transport,
                "failure.json",
                &json!({"format_version":1,"status":"object_observation_failed",
                    "stage":observation.stage,"binding":input_binding,"remote_writes":0,
                    "clone_verified":false,"restore_qualified":false}),
            );
            let detail = if retained.is_ok() {
                "保留已写证据"
            } else {
                "无法安全发布失败收据，保留已有文件"
            };
            Err(format!(
                "当前目标对象观察未通过（{}），{detail}，禁止重试：{}",
                observation.stage,
                observation.request.directory.display()
            )
            .into())
        }
    }
}

impl ObserveTarget<'_> {
    fn intent(&self) -> Result<Value> {
        common::scope_id(self.scope)?;
        validate_key(self.scope, self.key, false)?;
        if !BUCKETS.contains(&self.bucket) {
            return Err("目标对象桶不在已登记五桶内".into());
        }
        binding::digest(&json!(self.plan_sha256), "目标对象观察计划摘要")?;
        document::exact(self.expected, &["bytes", "sha256"], "目标对象计划载荷")?;
        binding::digest(&self.expected["sha256"], "目标对象计划载荷摘要")?;
        if self.expected["bytes"]
            .as_u64()
            .is_none_or(|bytes| bytes > self.max_bytes)
        {
            return Err("目标对象计划载荷大小无效或超过已登记上限".into());
        }
        let selected =
            app_configuration::public_configuration(self.configuration_root, self.configuration)?;
        if selected != json!({"endpoint":self.endpoint,"region":self.region}) {
            return Err("目标对象端点或区域与已绑定配置不同".into());
        }
        Ok(
            json!({"format_version":1,"kind":"devex-clone-object-capture","side":"target",
            "scope_id":self.scope,"endpoint":self.endpoint,"region":self.region,"bucket":self.bucket,
            "key":self.key,"expected":self.expected,"max_bytes":self.max_bytes,
            "transport":"rust-s3-sigv4","clone_verified":false}),
        )
    }
}

struct Observation<'a, T> {
    request: ObserveTarget<'a>,
    transport: &'a T,
    checkpoint: &'a dyn Fn() -> Result<()>,
    evidence: Evidence<'a>,
    configuration_inputs: Option<Inputs>,
    stage: &'static str,
}

impl<T: Transport> Observation<'_, T> {
    fn run(&mut self, capture_intent: &Value, input_binding: &Value) -> Result<Observed> {
        let mut intent = capture_intent.clone();
        intent["kind"] = json!("devex-clone-live-object");
        intent["binding"] = input_binding.clone();
        self.evidence
            .write(self.transport, "intent.json", &intent)?;
        self.configuration_inputs = Some(self.hold_configuration()?);
        let before = self.owner("owner-before")?;
        let first = self.head("head-before", None, None)?;
        let (after, capture) = if not_found(&first) {
            let after = self.owner("owner-after")?;
            if !not_found(&self.head("head-after", None, None)?) {
                return Err("目标对象在两次明确缺失观察之间出现".into());
            }
            (after, None)
        } else {
            let (after, captured) = self.capture(capture_intent, before.clone(), &first)?;
            (after, Some(captured))
        };
        self.stage = "binding-after";
        self.configuration()?;
        let capture_descriptor = match &capture {
            Some(value) => {
                self.evidence.check()?;
                self.evidence.write(self.transport, "capture.json", value)?;
                self.evidence.descriptor("capture.json")?
            }
            None => Value::Null,
        };
        let receipt = json!({"format_version":1,
            "status":if capture.is_some() {"object_present"} else {"object_absent"},
            "resource":{"kind":"object","scope_id":self.request.scope,"endpoint":self.request.endpoint,
                "bucket":self.request.bucket,"key":self.request.key},
            "binding":input_binding,"evidence":self.evidence.bindings(),
            "ownership_before":[before],"ownership_after":[after],"capture":capture_descriptor,
            "remote_writes":0,"clone_verified":false,"restore_qualified":false});
        self.stage = "publish";
        self.evidence.check()?;
        self.evidence
            .write(self.transport, "observation.json", &receipt)?;
        self.configuration()?;
        self.evidence.check()?;
        let descriptor = self.evidence.descriptor("observation.json")?;
        let raw_sha256 = self.evidence.files["observation.json"].sha256.clone();
        Ok(Observed {
            receipt,
            descriptor,
            raw_sha256,
            capture,
        })
    }

    fn hold_configuration(&self) -> Result<Inputs> {
        // 先由公共加载器核对精确 TOML 文件名和摘要，再从同一绑定持有保护句柄。
        Configuration::from_app_binding(
            self.request.configuration_root,
            self.request.configuration,
        )?;
        let directory = PathBuf::from(document::text(
            &self.request.configuration["directory"],
            "目标对象配置目录",
        )?);
        let files = document::object(&self.request.configuration["files"], "目标对象配置文件")?;
        Inputs::open(files.keys().map(|name| directory.join(name)))
    }

    fn configuration(&self) -> Result<Configuration> {
        (self.checkpoint)()?;
        self.evidence.check_directory()?;
        self.configuration_inputs
            .as_ref()
            .ok_or("目标对象观察缺少配置保护")?
            .checkpoint()?;
        Configuration::from_app_binding(self.request.configuration_root, self.request.configuration)
    }

    fn owner(&mut self, stage: &'static str) -> Result<Value> {
        let scope = self.request.scope;
        let bucket = self.request.bucket;
        let filename = format!("{stage}-{bucket}.bin");
        let key = format!("{scope}/.ryframe-owner");
        let response = self.call(
            stage,
            Operation::Get {
                key: key.clone(),
                if_match: None,
                version: None,
                destination: self.evidence.path.join(&filename),
                max_bytes: 4096,
            },
        )?;
        let owner = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
        if response["ContentLength"].as_u64() != Some(owner.len() as u64) {
            return Err("目标对象 owner 长度不符".into());
        }
        let actual = self.evidence.record_body(&filename, owner.len() as u64)?;
        if actual.sha256 != hex::encode(Sha256::digest(owner.as_bytes())) {
            return Err("目标对象 owner 不属于明确计划 scope".into());
        }
        Ok(json!({"bucket":bucket,"key":key,"file":filename,
            "bytes":actual.bytes,"sha256":actual.sha256}))
    }

    fn head(
        &mut self,
        stage: &'static str,
        if_match: Option<String>,
        version: Option<String>,
    ) -> Result<Value> {
        self.call(
            stage,
            Operation::Head {
                key: self.request.key.to_owned(),
                if_match,
                version,
            },
        )
    }

    fn capture(&mut self, intent: &Value, before: Value, first: &Value) -> Result<(Value, Value)> {
        let inspected = inspect_object(first)?;
        let identity = &inspected["identity"];
        if identity["ContentLength"] != self.request.expected["bytes"] {
            return Err("目标对象 HEAD 长度与已登记载荷不同".into());
        }
        let if_match = Some(document::text(&identity["ETag"], "目标对象 ETag")?.to_owned());
        let version = identity["VersionId"].as_str().map(str::to_owned);
        let get = self.call(
            "get",
            Operation::Get {
                key: self.request.key.to_owned(),
                if_match: if_match.clone(),
                version: version.clone(),
                destination: self.evidence.path.join("object.bin"),
                max_bytes: self.request.max_bytes,
            },
        )?;
        let bytes = self.request.expected["bytes"]
            .as_u64()
            .ok_or("对象大小无效")?;
        let actual = self.evidence.record_body("object.bin", bytes)?;
        let repeated = self.head("head-after", if_match, version)?;
        let after = self.owner("owner-after")?;
        self.stage = "capture";
        let capture = object_capture_result(
            intent,
            first,
            &get,
            &repeated,
            &json!({"bytes":actual.bytes,"sha256":actual.sha256}),
            vec![before],
            vec![after.clone()],
        )?;
        Ok((after, capture))
    }

    fn call(&mut self, stage: &'static str, operation: Operation) -> Result<Value> {
        self.stage = stage;
        let configuration = self.configuration()?;
        let head = matches!(operation, Operation::Head { .. });
        let operation_name = if head { "head-object" } else { "get-object" };
        if matches!(&operation, Operation::Get { key, .. } if key.as_str() == self.request.key) {
            self.evidence.check()?;
        }
        let response = self.transport.s3(Request {
            configuration,
            bucket: self.request.bucket.to_owned(),
            operation,
        });
        let checked = self.configuration();
        let failed = response.is_err();
        // 即使请求后的外层检查失败，也在同一目录保留已取得的原始响应。
        if let Ok(raw) = &response {
            self.evidence
                .write(self.transport, &format!("{stage}.json"), raw)?;
        }
        self.evidence.write(
            self.transport,
            &format!("{stage}.diagnostic.json"),
            &json!({"operation":operation_name,"returned":!failed,"checkpoint_passed":checked.is_ok()}),
        )?;
        checked?;
        let raw = response.map_err(|_| "原生 S3 只读请求失败")?;
        document::object(&raw, "原生目标对象响应")?;
        if !head && raw.get("status").is_some() {
            return Err("目标对象 GET 未取得成功响应".into());
        }
        Ok(raw)
    }
}

fn not_found(raw: &Value) -> bool {
    *raw == json!({"status":"not_found","http_status":404,"remote_write_may_have_occurred":false})
}

struct Evidence<'a> {
    backend: &'a Path,
    path: &'a Path,
    identity: Value,
    files: BTreeMap<String, FileDigest>,
}

impl<'a> Evidence<'a> {
    fn new(backend: &'a Path, work: &Path, path: &'a Path) -> Result<Self> {
        common::local_directory(backend, work, "目标对象观察工作根")?;
        if strip_local_test_prefix(path, work)
            .is_none_or(|relative| relative.as_os_str().is_empty())
        {
            return Err("目标对象证据目录须严格位于本次受管工作根内".into());
        }
        common::ensure_new_directory(backend, path, "目标对象观察目录")?;
        common::local_directory(backend, path, "目标对象观察目录")?;
        let (device, inode) = common::device_inode_for(path, &fs::symlink_metadata(path)?)?;
        Ok(Self {
            backend,
            path,
            identity: json!({"path":path,"device":device,"inode":inode}),
            files: BTreeMap::new(),
        })
    }

    fn check_directory(&self) -> Result<()> {
        common::directory_identity(self.backend, &self.identity)?;
        Ok(())
    }

    fn write(&mut self, transport: &impl Transport, name: &str, value: &Value) -> Result<()> {
        self.check_directory()?;
        let mut bytes = serde_json::to_vec_pretty(value)?;
        bytes.push(b'\n');
        transport.publish_evidence(&self.path.join(name), &bytes)?;
        let actual = self.record_body(name, bytes.len() as u64)?;
        if actual.sha256 != hex::encode(Sha256::digest(&bytes)) {
            return Err("目标对象 JSON 原始证据在发布期间变化".into());
        }
        Ok(())
    }

    fn record_body(&mut self, name: &str, expected_bytes: u64) -> Result<FileDigest> {
        self.check_directory()?;
        let path = common::common_file(self.backend, &self.path.join(name), "目标对象原始证据")?;
        if fs::metadata(&path)?.len() != expected_bytes {
            return Err("目标对象原始证据大小不同".into());
        }
        let actual = digest::file(&path)?;
        if actual.bytes != expected_bytes
            || self.files.insert(name.to_owned(), actual.clone()).is_some()
        {
            return Err("目标对象原始证据大小变化或重复登记".into());
        }
        Ok(actual)
    }

    fn bindings(&self) -> BTreeMap<&str, Value> {
        self.files
            .iter()
            .map(|(name, digest)| {
                (
                    name.as_str(),
                    json!({"bytes":digest.bytes,"sha256":digest.sha256}),
                )
            })
            .collect()
    }

    fn descriptor(&self, name: &str) -> Result<Value> {
        let digest = self.files.get(name).ok_or("目标对象收据缺少原始证据绑定")?;
        manifest::binding(&self.path.join(name), digest)
    }

    fn check(&self) -> Result<()> {
        self.check_directory()?;
        for entry in fs::read_dir(self.path)? {
            let name = entry?.file_name();
            if name
                .to_str()
                .is_none_or(|name| !self.files.contains_key(name))
            {
                return Err("目标对象目录包含失败标记或未登记文件".into());
            }
        }
        for (name, expected) in &self.files {
            let path =
                common::common_file(self.backend, &self.path.join(name), "目标对象原始证据")?;
            if fs::metadata(&path)?.len() != expected.bytes || digest::file(&path)? != *expected {
                return Err("目标对象原始证据已改变".into());
            }
        }
        self.check_directory()?;
        reject_failure(self.path)
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/observe_object.rs"]
mod tests;
