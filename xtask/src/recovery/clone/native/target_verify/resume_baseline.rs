//! 重读登记快照并验证 prepare 的所有观察；不信任调用方提供的文件清单。
use super::{
    super::{digest, json as json_io},
    evidence_model::*,
    fresh::selection,
    prepare_groups, prepare_observations, resume_observations as observations,
};
use crate::Result;
use serde_json::{Value, json};
#[cfg(windows)]
use std::path::PathBuf;
use std::{collections::BTreeSet, path::Path};

#[derive(Clone, Copy)]
pub(super) enum HistorySource<'a> {
    Original(Option<&'a Path>),
    #[cfg(windows)]
    Held {
        storage: &'a Path,
        history: &'a HeldPrepareHistory,
    },
}

#[cfg(windows)]
pub(in crate::recovery::clone::native) struct HeldPrepareHistory {
    backend: PathBuf,
    target: PathBuf,
    storage: PathBuf,
    request: Value,
    candidates: Vec<selection::Selected>,
    fence: crate::recovery::runtime::dataset_authority::reads::history::Fence,
}

#[cfg(windows)]
impl HeldPrepareHistory {
    pub(super) fn open(
        backend: &Path,
        target: &Path,
        request: &Value,
        storage: &Path,
    ) -> Result<Option<Self>> {
        use crate::recovery::runtime::dataset_authority::reads;

        if reads::is_active()
            || !selection::supports_held_history(backend, target, request, storage)?
        {
            return Ok(None);
        }
        let (candidates, fence) =
            reads::history::Fence::capture(|| selection::history(backend, target, request))?;
        fence.check()?;
        Ok(Some(Self {
            backend: backend.to_path_buf(),
            target: target.to_path_buf(),
            storage: storage.to_path_buf(),
            request: request.clone(),
            candidates,
            fence,
        }))
    }

    fn checkpoint(
        &self,
        backend: &Path,
        target: &Path,
        request: &Value,
        storage: &Path,
    ) -> Result<()> {
        check(
            self.backend == backend
                && self.target == target
                && self.request == *request
                && self.storage == storage,
            "prepare 持有的存储历史不属于当前后端、目标、固定请求或存储目录",
        )?;
        self.fence.checkpoint()
    }

    pub(in crate::recovery::clone::native) fn check_end(&self) -> Result<()> {
        self.fence.check()
    }
}

pub(super) fn prepared(
    backend: &Path,
    target: &Path,
    evidence: &Value,
    descriptor: &Value,
) -> Result<Value> {
    prepared_with_history(
        backend,
        target,
        evidence,
        descriptor,
        HistorySource::Original(None),
    )
}

pub(super) fn prepared_with_history(
    backend: &Path,
    target: &Path,
    evidence: &Value,
    descriptor: &Value,
    source: HistorySource<'_>,
) -> Result<Value> {
    json_io::exact(
        evidence,
        &["descriptor", "registration", "predecessor"],
        "prepare 文件绑定",
    )?;
    let path = bound(backend, &evidence["descriptor"])?;
    let snapshot = read(backend, &path)?;
    json_io::exact(
        &snapshot,
        &[
            "format_version",
            "kind",
            "registration",
            "predecessor",
            "files",
        ],
        "prepare 文件快照",
    )?;
    let registration_path = bound(backend, &evidence["registration"])?;
    let registration = read(backend, &registration_path)?;
    json_io::exact(
        &registration,
        &[
            "format_version",
            "kind",
            "request",
            "environment",
            "storage_run",
            "target_directory",
        ],
        "目标登记",
    )?;
    check(
        path.file_name()
            .is_some_and(|name| name == "prepared-files.json")
            && registration_path == path.with_file_name("registration.json")
            && path.parent() == target.parent()
            && snapshot["format_version"] == 1
            && snapshot["kind"] == "devex-clone-fresh-target-prepared-files"
            && snapshot["registration"] == evidence["registration"]
            && snapshot["predecessor"].is_null()
            && evidence["predecessor"].is_null()
            && registration["format_version"] == 1
            && registration["kind"] == "devex-clone-fresh-target-registration"
            && registration["request"] == *descriptor
            && Path::new(json_io::text(
                &registration["target_directory"],
                "目标目录",
            )?) == target,
        "fresh 目标 prepare 文件快照、registration 或 predecessor 绑定无效",
    )?;
    let request = read(backend, &bound(backend, descriptor)?)?;
    validate_with_history(
        backend,
        target,
        &snapshot["files"],
        descriptor,
        &request,
        source,
    )?;
    check(
        binding(backend, &path)? == evidence["descriptor"]
            && binding(backend, &registration_path)? == evidence["registration"],
        "fresh 目标 prepare 文件快照或 registration 在读取期间变化",
    )?;
    Ok(snapshot["files"].clone())
}

pub(super) fn document(
    backend: &Path,
    target: &Path,
    prepared: &Value,
    descriptor: &Value,
    request: &Value,
) -> Result<()> {
    document_with_history(backend, prepared, descriptor, request, || {
        selection::history(backend, target, request)
    })
    .map(|_| ())
}

fn document_with_history(
    backend: &Path,
    prepared: &Value,
    descriptor: &Value,
    request: &Value,
    history: impl FnOnce() -> Result<Vec<selection::Selected>>,
) -> Result<Vec<selection::Selected>> {
    json_io::exact(prepared, super::resume_model::PREPARE, "prepare")?;
    check(
        read(backend, &bound(backend, descriptor)?)? == *request,
        "prepare 固定请求与调用方正文不同",
    )?;
    let generation = &prepared["generation"];
    let candidates = history()?;
    check(
        candidates
            .iter()
            .any(|item| item.value == generation["storage"]),
        "prepare 存储代次未绑定固定请求的完整合法历史",
    )?;
    json_io::exact(
        generation,
        &[
            "execution",
            "maintenance",
            "physical",
            "configuration",
            "configuration_sha256",
            "seed_credentials_sha256",
            "request_sha256",
            "review_sha256",
            "selected",
            "operator_declared_controlled_generation_only",
            "external_writers_discovered",
            "storage",
        ],
        "prepare generation",
    )?;
    let maintenance = read(backend, &bound(backend, &request["maintenance_build"])?)?;
    let selected = super::resume_model::selected(backend, request)?;
    let intents = json_io::array(&request["target"]["databases"], "目标数据库")?
        .iter()
        .map(
            |db| json!({"key":db["key"],"server_uuid":db["server_uuid"],"database":db["database"]}),
        )
        .collect::<Vec<_>>();
    let databases = json_io::array(&request["target"]["databases"],"目标数据库")?.iter().map(|db|json!({"server_uuid":db["server_uuid"],"database":db["database"],"exists":false,"empty_checked":false})).collect::<Vec<_>>();
    let objects = BUCKETS
        .into_iter()
        .map(|bucket| (bucket.to_owned(), json!({"keys":[]})))
        .collect::<serde_json::Map<_, _>>();
    check(
        prepared["format_version"] == 1
            && prepared["status"] == "fresh_creation_prepared"
            && timestamp(&prepared["prepared_at"]).is_ok()
            && prepared["request"] == *descriptor
            && prepared["request_sha256"] == digest::plan_hash(request)?
            && prepared["fresh_target_initialized"] == false
            && prepared["controlled_generation_only"] == true
            && prepared["external_writers_discovered"] == false
            && prepared["create_intents"] == json!(intents)
            && prepared["absence"]
                == json!({"databases":databases,"objects":objects,"redis":{"keys":[],"owner":null,"sentinel":null}})
            && generation["maintenance"] == maintenance
            && generation["configuration_sha256"] == request["configuration_sha256"]
            && generation["request_sha256"] == digest::plan_hash(request)?
            && generation["review_sha256"] == request["review"]["sha256"]
            && generation["selected"] == selected
            && generation["operator_declared_controlled_generation_only"] == true
            && generation["external_writers_discovered"] == false
            && ["execution", "physical", "configuration"]
                .iter()
                .all(|key| generation[*key].is_object()),
        "fresh prepare 声明、来源或空资源前像无效",
    )?;
    Ok(candidates)
}

pub(crate) fn validate(
    backend: &Path,
    target: &Path,
    files: &Value,
    descriptor: &Value,
    request: &Value,
) -> Result<()> {
    validate_with_history(
        backend,
        target,
        files,
        descriptor,
        request,
        HistorySource::Original(None),
    )
}

fn validate_with_history(
    backend: &Path,
    target: &Path,
    files: &Value,
    descriptor: &Value,
    request: &Value,
    source: HistorySource<'_>,
) -> Result<()> {
    let values = bound_files(backend, target, files)?;
    let names = values.keys().cloned().collect::<BTreeSet<_>>();
    let consumed = BTreeSet::from(["request.json".to_owned(), "prepare.json".to_owned()]);
    check(
        consumed.is_subset(&names) && read(backend, &target.join("request.json"))? == *request,
        "fresh prepare 快照缺少固定请求或结果",
    )?;
    let prepared = read(backend, &target.join("prepare.json"))?;
    let mut history = History::new(backend, target, request, source);
    let candidates =
        document_with_history(backend, &prepared, descriptor, request, || history.first())?;
    if names.contains("failure.json") {
        super::prepare_state::failure(backend, target)?;
    }
    let records = super::evidence_prepare::records(backend, target, descriptor)?;
    check(
        records["pending"] == json!([]),
        "fresh prepare 存在没有显式接管的中断尝试",
    )?;
    let groups = prepare_groups::partition(backend, target, &names, &records)?;
    if names.contains("failure.json")
        || !json_io::object(&records["intents"], "prepare intents")?.is_empty()
    {
        check(
            groups.last().is_some_and(|group| group.confirmed),
            "fresh prepare 失败续作谱系没有完整确认",
        )?;
    }
    let candidates = history.second(candidates)?;
    validate_groups(backend, target, request, groups, &candidates)
}

fn validate_groups(
    backend: &Path,
    target: &Path,
    request: &Value,
    groups: Vec<prepare_groups::Group>,
    candidates: &[selection::Selected],
) -> Result<()> {
    let mut complete = false;
    for group in groups {
        let published = read(backend, &target.join("prepare.json"))?;
        let mut matches = BTreeSet::new();
        let mut best = None;
        let mut furthest = 0;
        for candidate in candidates {
            if group.published && candidate.value != published["generation"]["storage"] {
                continue;
            }
            let mut progress = 0;
            match prepare_observations::validate(
                backend,
                target,
                &group.files,
                request,
                candidate,
                group.complete,
                &mut progress,
            ) {
                Ok(()) => {
                    matches.insert(digest::plan_hash(&candidate.value)?);
                }
                Err(error) if best.is_none() || progress > furthest => {
                    furthest = progress;
                    best = Some(error);
                }
                Err(_) => {}
            }
        }
        if matches.is_empty() {
            return Err(best.unwrap_or_else(|| "prepare 观察没有合法完整账本代次".into()));
        }
        check(
            !group.complete || matches.len() == 1,
            "prepare 完整观察没有唯一账本代次",
        )?;
        complete |= group.complete;
    }
    check(complete, "prepare 缺少完整只读观察组")
}

struct History<'a> {
    backend: &'a Path,
    target: &'a Path,
    request: &'a Value,
    #[cfg(windows)]
    source: HistorySource<'a>,
    #[cfg(windows)]
    fence: Option<crate::recovery::runtime::dataset_authority::reads::history::Fence>,
}

impl<'a> History<'a> {
    fn new(
        backend: &'a Path,
        target: &'a Path,
        request: &'a Value,
        source: HistorySource<'a>,
    ) -> Self {
        #[cfg(not(windows))]
        let HistorySource::Original(_storage) = source;
        Self {
            backend,
            target,
            request,
            #[cfg(windows)]
            source,
            #[cfg(windows)]
            fence: None,
        }
    }

    fn first(&mut self) -> Result<Vec<selection::Selected>> {
        #[cfg(windows)]
        {
            use crate::recovery::runtime::dataset_authority::reads;
            match self.source {
                HistorySource::Held { storage, history } => {
                    history.checkpoint(self.backend, self.target, self.request, storage)?;
                    return Ok(history.candidates.clone());
                }
                HistorySource::Original(Some(storage))
                    if !reads::is_active()
                        && selection::supports_held_history(
                            self.backend,
                            self.target,
                            self.request,
                            storage,
                        )? =>
                {
                    let (selected, fence) = reads::history::Fence::capture(|| {
                        selection::history(self.backend, self.target, self.request)
                    })?;
                    self.fence = Some(fence);
                    return Ok(selected);
                }
                HistorySource::Original(_) => {}
            }
        }
        selection::history(self.backend, self.target, self.request)
    }

    fn second(self, first: Vec<selection::Selected>) -> Result<Vec<selection::Selected>> {
        #[cfg(windows)]
        {
            if let HistorySource::Held { storage, history } = self.source {
                history.checkpoint(self.backend, self.target, self.request, storage)?;
                return Ok(first);
            }
            if let Some(fence) = self.fence {
                fence.check()?;
                return Ok(first);
            }
        }
        let _ = first;
        selection::history(self.backend, self.target, self.request)
    }
}

fn bound_files(
    backend: &Path,
    target: &Path,
    files: &Value,
) -> Result<std::collections::BTreeMap<String, Value>> {
    let values = observations::file_map(files)?;
    for (name, value) in &values {
        check(
            value["type"] == "file" && !name.contains('/'),
            "fresh prepare 快照包含目录、越界路径或变化文件",
        )?;
        check(
            *value == {
                let actual = binding(backend, &target.join(name))?;
                json!({"path":name,"type":"file","bytes":actual["bytes"],"sha256":actual["sha256"]})
            },
            "fresh prepare 快照包含目录、越界路径或变化文件",
        )?;
    }
    Ok(values)
}

/// 初始化消费者仍保留整个历史树，仅用最近一次完整观察组作资源比较基线。
pub(crate) fn active(
    backend: &Path,
    target: &Path,
    files: &Value,
    request: &Value,
) -> Result<Value> {
    let prepared = read(backend, &target.join("prepare.json"))?;
    validate(backend, target, files, &prepared["request"], request)?;
    let names = observations::file_map(files)?
        .into_keys()
        .collect::<BTreeSet<_>>();
    let records = super::evidence_prepare::records(backend, target, &prepared["request"])?;
    let groups = prepare_groups::partition(backend, target, &names, &records)?;
    let active = groups
        .iter()
        .rev()
        .find(|group| group.complete)
        .ok_or("prepare 没有完整观察组")?;
    Ok(Value::Array(
        json_io::array(files, "prepare 完整历史树")?
            .iter()
            .filter(|file| {
                file["path"]
                    .as_str()
                    .is_some_and(|name| active.files.contains(name))
            })
            .cloned()
            .collect(),
    ))
}
