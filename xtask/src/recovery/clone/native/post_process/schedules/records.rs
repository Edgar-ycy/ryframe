//! 调度收据固定原路径与字节，独占新建各次意图、响应和核对记录。

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

use super::{ReadPort, Settings, inspect_disabled, validate_api_row, validate_before, version};
use crate::{
    Result,
    recovery::clone::native::{
        json as document,
        resources::common,
        target_verify::{guard::Inputs, publication},
    },
};

const INTENT: &str = "intent.json";
const BEFORE: &str = "before.json";
const API_BEFORE: &str = "api-before.json";
const CONFIRMED: &str = "confirmed.json";
const ACTION_EVIDENCE: [&str; 3] = [INTENT, BEFORE, API_BEFORE];

pub(super) fn present(backend: &Path, path: &Path) -> Result<bool> {
    if exists(path)? {
        common::common_file(backend, path, "调度证据")?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub(super) fn directories(backend: &Path, root: &Path) -> Result<Vec<PathBuf>> {
    if !exists(root)? {
        return Ok(Vec::new());
    }
    common::local_directory(backend, root, "调度证据根目录")?;
    let mut entries = entries(root)?;
    entries.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<usize>().ok())
    });
    for (index, path) in entries.iter().enumerate() {
        let expected = format!("{:04}", index + 1);
        if path.file_name().and_then(|name| name.to_str()) != Some(expected.as_str()) {
            return Err("调度操作证据目录不连续或包含未知文件".into());
        }
        common::local_directory(backend, path, "既有调度操作目录")?;
    }
    Ok(entries)
}

pub(super) fn create_root(backend: &Path, path: &Path) -> Result<()> {
    if !exists(path)? {
        fs::create_dir(path)?;
    }
    common::local_directory(backend, path, "调度证据根目录").map(|_| ())
}

pub(super) fn check_preparation(backend: &Path, directory: &Path) -> Result<()> {
    for entry in entries(directory)? {
        if !matches!(
            entry.file_name().and_then(|name| name.to_str()),
            Some(BEFORE | API_BEFORE)
        ) {
            return Err("未登记请求的目录包含未知写入证据".into());
        }
        common::common_file(backend, &entry, "未发送请求的调度准备文件")?;
    }
    Ok(())
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn entries(directory: &Path) -> Result<Vec<PathBuf>> {
    fs::read_dir(directory)?
        .map(|entry| Ok(entry?.path()))
        .collect()
}

pub(super) struct Intent<'a, 's> {
    pub(super) settings: &'a Settings<'s>,
    pub(super) action: &'a Value,
    pub(super) before: Value,
    directory: &'a Path,
    binding: Value,
    frozen: Inputs,
}

impl<'a, 's> Intent<'a, 's> {
    pub(super) fn load(
        settings: &'a Settings<'s>,
        action: &'a Value,
        directory: &'a Path,
    ) -> Result<Self> {
        let path = common::common_file(settings.backend, &directory.join(INTENT), "调度意图")?;
        let frozen = Inputs::open([
            path.clone(),
            directory.join(BEFORE),
            directory.join(API_BEFORE),
        ])?;
        let binding = common::binding(&path)?;
        let value = document::read(&path)?;
        document::exact(
            &value,
            &[
                "registration",
                "action",
                "before",
                "api_before",
                "operation",
                "automatic_retry",
            ],
            "调度意图",
        )?;
        if value["action"] != *action
            || value["registration"] != *settings.registration
            || value["operation"] != "put_monitor_schedules_by_id_status"
            || value["automatic_retry"].as_bool() != Some(false)
        {
            return Err("调度意图不属于当前登记的动作".into());
        }
        let before = read_expected(settings.backend, directory, BEFORE, &value["before"])?;
        validate_before(settings.execution, action, &before)?;
        let api = read_expected(
            settings.backend,
            directory,
            API_BEFORE,
            &value["api_before"],
        )?;
        validate_api_row(&api, action, true, version(action, false)?)?;
        frozen.check()?;
        Ok(Self {
            settings,
            action,
            before,
            directory,
            binding,
            frozen,
        })
    }

    pub(super) fn prepare(
        settings: &'a Settings<'s>,
        action: &'a Value,
        directory: &'a Path,
        before: &Value,
        api_before: &Value,
        port: &impl ReadPort,
    ) -> Result<Self> {
        publication::publish(&directory.join(BEFORE), before)?;
        publication::publish(&directory.join(API_BEFORE), api_before)?;
        port.checkpoint()?;
        publication::publish(
            &directory.join(INTENT),
            &json!({
                "registration":settings.registration,"action":action,
                "before":common::binding(&directory.join(BEFORE))?,
                "api_before":common::binding(&directory.join(API_BEFORE))?,
                "operation":"put_monitor_schedules_by_id_status","automatic_retry":false
            }),
        )?;
        let intent = Self::load(settings, action, directory)?;
        if intent.before != *before || document::read(&directory.join(API_BEFORE))? != *api_before {
            return Err("调度意图记录的前像在写入前发生变化".into());
        }
        intent.check()?;
        port.checkpoint()?;
        Ok(intent)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.frozen.check()
    }

    pub(super) fn confirmed(&self) -> Result<Confirmation> {
        let path = self.directory.join(CONFIRMED);
        common::common_file(self.settings.backend, &path, "调度确认")?;
        let confirmation_file = Inputs::open([path.clone()])?;
        let binding = common::binding(&path)?;
        let value = document::read(&path)?;
        document::exact(
            &value,
            &[
                "registration",
                "action",
                "observed",
                "api_execution_proven",
                "evidence",
            ],
            "调度确认",
        )?;
        if value["registration"] != *self.settings.registration || value["action"] != *self.action {
            return Err("调度确认不属于当前动作".into());
        }
        let proven = value["api_execution_proven"]
            .as_bool()
            .ok_or("调度确认缺少明确 API 执行证明")?;
        let evidence = document::object(&value["evidence"], "调度确认证据")?;
        let (name, names) = proof_files(proven, evidence)?;
        let frozen = Inputs::open(names.iter().map(|name| self.directory.join(name)))?;
        for entry in &names {
            expected(
                self.settings.backend,
                self.directory,
                entry,
                &evidence[*entry],
            )?;
        }
        let record = document::read(&self.directory.join(name))?;
        let observed = if proven {
            let response = document::read(&self.directory.join("api-response.json"))?;
            validate_api_row(&response, self.action, false, version(self.action, true)?)?;
            &record
        } else {
            self.reconciled(&record)?
        };
        if value["observed"] != *observed || evidence[INTENT] != self.binding {
            return Err("调度确认内嵌行与原始观察不同".into());
        }
        self.validate_observation(observed, true)?;
        self.check()?;
        frozen.check()?;
        confirmation_file.check()?;
        Ok(Confirmation {
            binding,
            inspection: observed["inspection"].clone(),
            frozen,
            confirmation_file,
        })
    }

    pub(super) fn allow_new_attempt(&self) -> Result<()> {
        let mut files = Vec::new();
        for path in entries(self.directory)? {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("调度证据名称无效")?;
            if name.starts_with("reconcile-") {
                files.push((
                    reconciliation_number(name).ok_or("调度核对文件名称无效")?,
                    path,
                ));
            }
        }
        files.sort_by_key(|(number, _)| *number);
        let (_, path) = files.last().ok_or("已有未知调度写入必须先显式核对")?;
        common::common_file(self.settings.backend, path, "调度核对")?;
        let frozen = Inputs::open([path.clone()])?;
        let record = document::read(path)?;
        self.validate_observation(self.reconciled(&record)?, false)?;
        frozen.check()?;
        self.check()
    }

    pub(super) fn save_reconciliation(&self, name: &str, observed: &Value) -> Result<()> {
        self.check()?;
        publication::publish(
            &self.directory.join(name),
            &json!({
                "registration":self.settings.registration,"intent":self.binding,"observed":observed
            }),
        )
    }

    pub(super) fn save_confirmation(
        &self,
        observed: &Value,
        response: Option<&str>,
        name: &str,
    ) -> Result<()> {
        self.check()?;
        self.validate_observation(observed, true)?;
        let names = ACTION_EVIDENCE.into_iter().chain([name]).chain(response);
        let mut evidence = Map::new();
        let mut paths = Vec::new();
        for name in names {
            let path = common::common_file(
                self.settings.backend,
                &self.directory.join(name),
                "调度确认原始证据",
            )?;
            paths.push((name, path));
        }
        let frozen = Inputs::open(paths.iter().map(|(_, path)| path.clone()))?;
        for (name, path) in paths {
            evidence.insert(name.to_owned(), common::binding(&path)?);
        }
        let record = document::read(&self.directory.join(name))?;
        let saved = if let Some(response) = response {
            validate_api_row(
                &document::read(&self.directory.join(response))?,
                self.action,
                false,
                version(self.action, true)?,
            )?;
            &record
        } else {
            self.reconciled(&record)?
        };
        if observed != saved || evidence[INTENT] != self.binding {
            return Err("调度确认的原始证据在发布前发生变化".into());
        }
        frozen.check()?;
        publication::publish(
            &self.directory.join(CONFIRMED),
            &json!({
                "registration":self.settings.registration,"action":self.action,
                "observed":observed,"api_execution_proven":response.is_some(),"evidence":evidence
            }),
        )
    }

    fn reconciled<'r>(&self, value: &'r Value) -> Result<&'r Value> {
        document::exact(value, &["registration", "intent", "observed"], "调度核对")?;
        if value["registration"] != *self.settings.registration || value["intent"] != self.binding {
            return Err("调度核对不属于当前意图".into());
        }
        Ok(&value["observed"])
    }

    fn validate_observation(&self, observed: &Value, disabled: bool) -> Result<()> {
        document::exact(observed, &["inspection", "current", "api"], "调度观察")?;
        let inspected = inspect_disabled(
            self.settings.execution,
            self.action,
            &self.before,
            &observed["current"],
        )?;
        let status = if disabled {
            "disabled_row_verified"
        } else {
            "unchanged_before"
        };
        if inspected != observed["inspection"] || inspected["status"] != status {
            return Err("调度核对未得到要求的完整行".into());
        }
        validate_api_row(
            &observed["api"],
            self.action,
            !disabled,
            version(self.action, disabled)?,
        )
    }
}

pub(super) struct Confirmation {
    pub(super) binding: Value,
    inspection: Value,
    frozen: Inputs,
    confirmation_file: Inputs,
}

impl Confirmation {
    pub(super) fn compare(&self, observed: &Value) -> Result<()> {
        if self.inspection != observed["inspection"] {
            return Err("已确认调度的完整行发生变化".into());
        }
        self.frozen.check()?;
        self.confirmation_file.check()
    }
}

fn proof_files(proven: bool, evidence: &Map<String, Value>) -> Result<(&str, Vec<&str>)> {
    let mut names = ACTION_EVIDENCE.to_vec();
    let name = if proven {
        names.push("api-response.json");
        "after.json"
    } else {
        evidence
            .keys()
            .map(String::as_str)
            .find(|name| reconciliation_number(name).is_some())
            .ok_or("调度确认缺少明确的核对证据")?
    };
    names.push(name);
    if names.len() != evidence.len() || names.iter().any(|name| !evidence.contains_key(*name)) {
        return Err("调度确认证据文件集合不匹配".into());
    }
    Ok((name, names))
}

fn reconciliation_number(name: &str) -> Option<u64> {
    let number = name
        .strip_prefix("reconcile-")?
        .strip_suffix(".json")?
        .parse::<u64>()
        .ok()?;
    (number > 0 && name == format!("reconcile-{number:04}.json")).then_some(number)
}

fn expected(backend: &Path, directory: &Path, name: &str, binding: &Value) -> Result<PathBuf> {
    let path = common::bound_file(backend, binding, "调度证据绑定")?;
    if path != directory.join(name) {
        return Err("调度证据引用了其他操作目录".into());
    }
    Ok(path)
}

fn read_expected(backend: &Path, directory: &Path, name: &str, binding: &Value) -> Result<Value> {
    document::read(&expected(backend, directory, name, binding)?)
}
