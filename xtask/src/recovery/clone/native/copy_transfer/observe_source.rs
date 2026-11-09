//! 按已核的原导出重新观察来源对象身份，不下载业务对象或改变远端资源。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    thread,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::{
    digest,
    export::transport::Transport,
    input::BUCKETS,
    json as document,
    lifecycle::manifest,
    resources::common,
    s3::{Configuration, Operation, Request, app_configuration, protocol::validate_key},
    target_verify::publication,
};
use crate::{
    Result,
    local_test_path::strip_local_test_prefix,
    recovery::{inputs::document as binding, source::evidence::inspect_object},
};

const MAX_PAGES: usize = 10_000;
const HEAD_WORKERS: usize = 4;

pub(crate) struct ObserveSource<'a> {
    pub(crate) backend: &'a Path,
    pub(crate) work: &'a Path,
    pub(crate) directory: &'a Path,
    pub(crate) export: &'a Value,
    pub(crate) captures: &'a BTreeMap<String, BTreeMap<String, Value>>,
    pub(crate) scope: &'a str,
    pub(crate) endpoint: &'a str,
    pub(crate) region: &'a str,
    pub(crate) configuration: &'a Value,
}

#[derive(Clone, Copy)]
pub(crate) enum Selection<'a> {
    One { bucket: &'a str, key: &'a str },
    All,
}

/// 外层检查须能从各个 HEAD 工作线程调用并复核同一原导出、代次与配置绑定。
pub(crate) fn observe<T: Transport + Sync>(
    request: ObserveSource<'_>,
    selection: Selection<'_>,
    transport: &T,
    checkpoint: &(dyn Fn() -> Result<()> + Sync),
) -> Result<Value> {
    request.validate(selection)?;
    common::local_directory(request.backend, request.work, "源对象观察工作根")?;
    if strip_local_test_prefix(request.directory, request.work)
        .is_none_or(|relative| relative.as_os_str().is_empty())
    {
        return Err("源对象证据须写入本次受管工作根内的新目录".into());
    }
    common::ensure_new_directory(request.backend, request.directory, "源对象观察目录")?;
    common::local_directory(request.backend, request.directory, "源对象观察目录")?;
    let reads = ReadWindow::new(
        request.backend,
        request.backend,
        request.directory,
        request.scope,
        request.configuration,
        transport,
        checkpoint,
    )?;
    let window = Observation { request, reads };
    let mut stage = "source_bindings";
    match window.run(selection, &mut stage) {
        Ok(result) => Ok(result),
        Err(_) => {
            let retained = window.reads.write(
                "failure.json",
                &json!({"status":"source_object_observation_failed","stage":stage,
                    "error_type":"SourceObjectObservationError",
                    "export_binding":window.request.export,"remote_writes":0,
                    "clone_verified":false,"restore_qualified":false}),
            );
            let detail = if retained.is_ok() {
                "保留已写证据"
            } else {
                "无法安全发布失败收据，保留已有文件"
            };
            Err(format!(
                "源对象只读观察未通过（{stage}），{detail}，禁止重试：{}",
                window.request.directory.display()
            )
            .into())
        }
    }
}

impl ObserveSource<'_> {
    fn validate(&self, selection: Selection<'_>) -> Result<()> {
        common::scope_id(self.scope)?;
        let export = manifest::validate_binding_shape(self.export, "源对象观察原导出")?;
        let path = Path::new(document::text(&export["path"], "源导出路径")?);
        if path.file_name().and_then(|name| name.to_str()) != Some("export.json")
            || self.configuration["app_env"] != "test"
            || self.captures.len() != BUCKETS.len()
            || BUCKETS
                .iter()
                .any(|bucket| !self.captures.contains_key(*bucket))
        {
            return Err("源对象观察须绑定原 export.json、test 配置与完整五桶目录".into());
        }
        common::common_file(self.backend, path, "源对象观察原导出")?;
        if let Selection::One { bucket, key } = selection {
            if self
                .captures
                .get(bucket)
                .is_none_or(|items| !items.contains_key(key))
            {
                return Err("源单对象必须来自已绑定的完整导出目录".into());
            }
            validate_key(self.scope, key, false)?;
        }
        let configuration =
            app_configuration::public_configuration(self.backend, self.configuration)?;
        if configuration != json!({"endpoint":self.endpoint,"region":self.region}) {
            return Err("源对象端点或区域与已登记 TOML 不同".into());
        }
        Ok(())
    }
}

struct Observation<'a, T> {
    request: ObserveSource<'a>,
    reads: ReadWindow<'a, T>,
}

impl<T: Transport + Sync> Observation<'_, T> {
    fn run(&self, selection: Selection<'_>, stage: &mut &'static str) -> Result<Value> {
        self.reads.configuration()?;
        let whole = matches!(selection, Selection::All);
        let selected = match selection {
            Selection::One { bucket, key } => json!([bucket, key]),
            Selection::All => Value::Null,
        };
        self.reads.write("intent.json", &json!({
            "export_binding":self.request.export,"selected":selected,"business_get_permitted":false,
            "binding":{"transport":"rust-s3-sigv4","scope_id":self.request.scope,
                "endpoint":self.request.endpoint,"region":self.request.region,
                "configuration_sha256":digest::plan_hash(&json!({"configuration":self.request.configuration}))?}
        }))?;
        let (buckets, entries) = match selection {
            Selection::One { bucket, key } => (vec![bucket], vec![(bucket, key)]),
            Selection::All => {
                let buckets = self.request.captures.keys().map(String::as_str).collect();
                let entries = self
                    .request
                    .captures
                    .iter()
                    .flat_map(|(bucket, items)| {
                        items.keys().map(move |key| (bucket.as_str(), key.as_str()))
                    })
                    .collect();
                (buckets, entries)
            }
        };
        *stage = "owners_before";
        self.reads.owners(&buckets, "owner-before")?;
        if whole {
            *stage = "complete_listing_before";
            self.listings(&buckets, "before")?;
        }
        *stage = "heads";
        let observed = if whole {
            self.heads(&entries)?
        } else {
            let (bucket, key) = entries[0];
            vec![self.head(0, bucket, key)?]
        };
        if whole {
            *stage = "complete_listing_after";
            self.listings(&buckets, "after")?;
        }
        *stage = "owners_after";
        self.reads.owners(&buckets, "owner-after")?;
        *stage = "binding_after";
        self.reads.configuration()?;
        let identity_sha256 = digest::plan_hash(&json!(observed))?;
        let result = json!({"status":"source_object_identities_observed","whole_source":whole,
            "export_binding":self.request.export,"observed":observed,"identity_sha256":identity_sha256,
            "business_get_requests":0,"business_bytes_downloaded":0,"source_body_sha_recomputed":false,
            "remote_writes":0,"clone_verified":false,"restore_qualified":false});
        *stage = "publish";
        self.reads.check_directory()?;
        let receipt = self.request.directory.join("observation.json");
        publication::publish(&receipt, &result)?;
        self.reads.configuration()?;
        if document::read(&receipt)? != result {
            return Err("源对象观察收据在发布期间变化".into());
        }
        Ok(result)
    }

    fn heads(&self, entries: &[(&str, &str)]) -> Result<Vec<Value>> {
        let mut rows = Vec::with_capacity(entries.len());
        for (batch, entries) in entries.chunks(HEAD_WORKERS).enumerate() {
            let results = thread::scope(|scope| {
                let handles = entries
                    .iter()
                    .enumerate()
                    .map(|(offset, &(bucket, key))| {
                        scope.spawn(move || {
                            self.head(batch * HEAD_WORKERS + offset, bucket, key)
                                .map_err(|_| "源对象当前 HEAD 未通过")
                        })
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|handle| handle.join().unwrap_or(Err("源对象 HEAD 工作线程未完成")))
                    .collect::<Vec<_>>()
            });
            for result in results {
                rows.push(result?);
            }
        }
        Ok(rows)
    }

    fn head(&self, index: usize, bucket: &str, key: &str) -> Result<Value> {
        validate_key(self.request.scope, key, false)?;
        let expected = &self.request.captures[bucket][key];
        let identity = &expected["identity"];
        let etag = document::text(&identity["ETag"], "原捕获 ETag")?;
        let version = match &identity["VersionId"] {
            Value::Null => None,
            value => Some(document::text(value, "原捕获 VersionId")?.to_owned()),
        };
        let original_sha =
            binding::digest(&expected["response_sha256"]["head_before"], "原 HEAD 摘要")?;
        let raw = self.reads.call(
            &format!("head-{index}"),
            "head-object",
            bucket,
            Operation::Head {
                key: key.to_owned(),
                if_match: Some(etag.to_owned()),
                version,
            },
        )?;
        let actual = inspect_object(&raw)?;
        let head_sha256 = digest::plan_hash(&raw)?;
        if actual["identity"] != *identity
            || actual["metadata"] != expected["metadata"]
            || head_sha256 != original_sha
        {
            return Err("源当前 HEAD 身份、元数据或完整原响应与已登记采集不同".into());
        }
        Ok(json!({"bucket":bucket,"key":key,"head_sha256":head_sha256,"identity":identity}))
    }

    fn listings(&self, buckets: &[&str], phase: &str) -> Result<()> {
        let owner_key = format!("{}/.ryframe-owner", self.request.scope);
        let mut matched = true;
        for &bucket in buckets {
            let expected = self.request.captures[bucket]
                .keys()
                .map(String::as_str)
                .chain([owner_key.as_str()])
                .collect::<BTreeSet<_>>();
            let current = self.reads.keys(bucket, phase)?;
            matched &= current.iter().map(String::as_str).collect::<BTreeSet<_>>() == expected;
        }
        if !matched {
            return Err("源五桶完整对象集合与已核原导出不同".into());
        }
        Ok(())
    }
}

pub(in crate::recovery) struct ReadWindow<'a, T> {
    backend: &'a Path,
    configuration_root: &'a Path,
    directory: &'a Path,
    scope: &'a str,
    configuration: &'a Value,
    identity: Value,
    transport: &'a T,
    checkpoint: &'a (dyn Fn() -> Result<()> + Sync),
}

impl<'a, T: Transport> ReadWindow<'a, T> {
    pub(in crate::recovery) fn new(
        backend: &'a Path,
        configuration_root: &'a Path,
        directory: &'a Path,
        scope: &'a str,
        configuration: &'a Value,
        transport: &'a T,
        checkpoint: &'a (dyn Fn() -> Result<()> + Sync),
    ) -> Result<Self> {
        let (device, inode) =
            common::device_inode_for(directory, &fs::symlink_metadata(directory)?)?;
        Ok(Self {
            backend,
            configuration_root,
            directory,
            scope,
            configuration,
            transport,
            checkpoint,
            identity: json!({"path":directory,"device":device,"inode":inode}),
        })
    }

    pub(in crate::recovery) fn keys(&self, bucket: &str, phase: &str) -> Result<BTreeSet<String>> {
        let prefix = format!("{}/", self.scope);
        let mut found = BTreeSet::new();
        let mut cursors = BTreeSet::new();
        let mut continuation = None;
        for index in 0..MAX_PAGES {
            let page = self.call(
                &format!("list-{phase}-{bucket}-{index}"),
                "list-objects-v2",
                bucket,
                Operation::List {
                    prefix: prefix.clone(),
                    continuation,
                },
            )?;
            let fields = document::object(&page, "对象分页")?;
            let truncated = page["IsTruncated"]
                .as_bool()
                .ok_or("对象分页缺少明确完成状态")?;
            if fields.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "IsTruncated" | "Contents" | "NextContinuationToken"
                )
            }) {
                return Err("对象分页包含未知字段".into());
            }
            if let Some(contents) = page.get("Contents") {
                for item in document::array(contents, "对象分页内容")? {
                    let key = document::text(&item["Key"], "对象分页 key")?;
                    validate_key(self.scope, key, true)?;
                    if !found.insert(key.to_owned()) {
                        return Err("对象分页发现重复 key".into());
                    }
                }
            }
            if !truncated {
                return Ok(found);
            }
            let cursor = document::text(&page["NextContinuationToken"], "对象分页游标")?;
            if cursor.is_empty() || !cursors.insert(cursor.to_owned()) {
                return Err("对象分页游标缺失或重复".into());
            }
            continuation = Some(cursor.to_owned());
        }
        Err("对象完整分页超过已登记页数上限".into())
    }

    pub(in crate::recovery) fn owners(&self, buckets: &[&str], phase: &str) -> Result<Vec<Value>> {
        let scope = self.scope;
        let mut rows = Vec::with_capacity(buckets.len());
        for &bucket in buckets {
            let stage = format!("{phase}-{bucket}");
            let body = self.directory.join(format!("{stage}.bin"));
            let raw = self.call(
                &stage,
                "get-object",
                bucket,
                Operation::Get {
                    key: format!("{scope}/.ryframe-owner"),
                    if_match: None,
                    version: None,
                    destination: body.clone(),
                    max_bytes: 4096,
                },
            )?;
            let expected = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
            let body = common::common_file(self.backend, &body, "对象桶原始 owner 证据")?;
            if raw.get("status").is_some()
                || raw["ContentLength"].as_u64() != Some(expected.len() as u64)
                || fs::metadata(&body)?.len() != expected.len() as u64
                || fs::read(&body)? != expected.as_bytes()
            {
                return Err("对象桶 owner 不属于已登记 scope".into());
            }
            rows.push(json!({"bucket":bucket,"key":format!("{scope}/.ryframe-owner"),
                "file":format!("{stage}.bin"),"bytes":expected.len(),"sha256":hex::encode(Sha256::digest(expected.as_bytes()))}));
        }
        Ok(rows)
    }

    pub(super) fn call(
        &self,
        stage: &str,
        name: &str,
        bucket: &str,
        operation: Operation,
    ) -> Result<Value> {
        let configuration = self.configuration()?;
        let response = self.transport.s3(Request {
            configuration,
            bucket: bucket.to_owned(),
            operation,
        });
        let checked = self.configuration();
        if let Ok(raw) = &response {
            self.write(&format!("{stage}.json"), raw)?;
        }
        self.write(&format!("{stage}.diagnostic.json"),
            &json!({"operation":name,"returned":response.is_ok(),"checkpoint_passed":checked.is_ok()}))?;
        checked?;
        response.map_err(|_| "原生 S3 只读请求失败".into())
    }

    pub(in crate::recovery::clone::native) fn configuration(&self) -> Result<Configuration> {
        (self.checkpoint)()?;
        self.check_directory()?;
        Configuration::from_app_binding(self.configuration_root, self.configuration)
    }

    pub(super) fn check_directory(&self) -> Result<()> {
        common::directory_identity(self.backend, &self.identity)?;
        Ok(())
    }

    pub(in crate::recovery::clone::native) fn write(
        &self,
        name: &str,
        value: &Value,
    ) -> Result<()> {
        self.check_directory()?;
        document::write_new(&self.directory.join(name), value)
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/observe_source.rs"]
mod tests;
