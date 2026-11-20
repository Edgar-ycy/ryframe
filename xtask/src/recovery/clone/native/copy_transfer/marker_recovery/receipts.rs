use std::{cell::Cell, fs, io::ErrorKind, path::PathBuf};

use serde_json::{Value, json};

use super::{
    super::super::{digest, json as document, resources::common, target_verify::guard::Inputs},
    Recovery, Stage, empty_image, protected_file,
};
use crate::Result;

#[path = "receipts/confirmed.rs"]
mod confirmed;

pub(super) struct Receipts {
    backend: PathBuf,
    pub(super) directory: PathBuf,
    intent: PathBuf,
    attempt: u64,
    binding: Value,
    head: Value,
    after: Value,
    controller: Value,
    run: PathBuf,
    protected: Inputs,
    prior_marker_root: Option<Inputs>,
    prior: Option<confirmed::Prior>,
}

impl Receipts {
    pub(super) fn open(recovery: &Recovery<'_>) -> Result<Self> {
        let stage = recovery.stage;
        let backend = stage.request.backend;
        let root = stage.request.run_dir.join("target-cache-markers");
        let digest = digest::plan_hash(&recovery.binding["fixture"])?;
        common::ensure_directory(backend, &root, "Redis 标记恢复目录")?;
        let directory = root.join(digest);
        common::ensure_directory(backend, &directory, "当前 Redis 代次的标记恢复目录")?;
        let protected = Inputs::directory(&directory)?;
        let lineage = &recovery.binding["fixture"]["lineage"];
        let prior_marker_root = if lineage.get("reboot_incident").is_some()
            || lineage.get("accepted_redis_failure").is_some()
        {
            Some(Inputs::directory(&root)?)
        } else {
            None
        };
        let mut receipts = Self {
            backend: backend.to_owned(),
            intent: directory.join("intent.json"),
            directory,
            attempt: stage.attempt,
            binding: recovery.binding.clone(),
            head: recovery.head.clone(),
            after: recovery.after.clone(),
            controller: stage.controller.clone(),
            run: stage.request.run_dir.to_owned(),
            protected,
            prior_marker_root,
            prior: None,
        };
        if let Some(incident) = lineage.get("accepted_redis_failure") {
            let (prior, proof) = confirmed::Prior::load(
                &receipts,
                &incident["previous_fixture_sha256"],
                |prefix| recovery.check_previous_prefix(prefix),
            )?;
            receipts.binding["previous_confirmed_markers"] = proof;
            receipts.prior = Some(prior);
        }
        receipts.check()?;
        Ok(receipts)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.protected.check()?;
        if let Some(prior) = &self.prior {
            prior.check()?;
        }
        if let Some(guard) = &self.prior_marker_root {
            guard.check()?;
            let root = self.directory.parent().ok_or("缺少标记逻辑运行目录")?;
            let allowed = self.prior.as_ref().map(confirmed::Prior::intent);
            reject_prior_intents(root, &self.directory, allowed, |path| {
                common::local_directory(&self.backend, path, "先前 Redis 标记代次目录")?;
                Ok(())
            })?;
            guard.check()?;
        }
        Ok(())
    }

    pub(super) fn require_unattempted(&self) -> Result<()> {
        match fs::symlink_metadata(&self.intent) {
            Ok(_) => Err("该 Redis 代次已有标记写入意图，请显式核对".into()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn unreadable(&self, checkpoint: &dyn Fn() -> Result<()>) -> Result<Value> {
        checkpoint()?;
        self.result("unknown", None, &Value::Null, 0, false)
    }

    pub(super) fn apply(
        &self,
        before: &Value,
        checkpoint: &dyn Fn() -> Result<()>,
        write: impl FnOnce(&dyn Fn() -> Result<()>) -> Result<Value>,
    ) -> Result<Value> {
        checkpoint()?;
        let observed = classify(before, &self.after);
        if observed != "before" {
            return self.result(observed, Some(before), &Value::Null, 0, observed == "after");
        }
        let evidence = json!({"format_version":1,"kind":"fixture_redis_marker_intent",
            "attempt":self.attempt,"controller":self.controller,"binding":self.binding,
            "copy_prefix":self.head,"before":before,"after":self.after});
        self.require_unattempted()?;
        document::write_new(&self.intent, &evidence)?;
        let intent = self.intent_binding()?;
        let held = Inputs::open([protected_file(&self.backend, &self.intent)?])?;
        let intent_ready = || {
            checkpoint()?;
            held.check()?;
            common::bound_file(&self.backend, &intent, "原 Redis 标记写入意图")?;
            Ok(())
        };
        let started = Cell::new(false);
        let begin_write = || {
            intent_ready()?;
            if started.replace(true) {
                return Err("当前 Redis 标记意图已发送，不允许再次调用".into());
            }
            Ok(())
        };
        let outcome = super::combine(write(&begin_write), intent_ready());
        let attempts = u8::from(started.get());
        match outcome {
            Ok(current) => {
                let classification = classify(&current, &self.after);
                self.result(
                    classification,
                    Some(&current),
                    &intent,
                    attempts,
                    classification == "after" && started.get(),
                )
            }
            Err(_) => self.result(
                if started.get() {
                    "unknown"
                } else {
                    "blocked_before_write"
                },
                None,
                &intent,
                attempts,
                false,
            ),
        }
    }

    pub(super) fn reconcile(
        &self,
        verify_prefix: impl FnOnce(&Value) -> Result<()>,
        observe: impl FnOnce() -> Result<Value>,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Value> {
        checkpoint()?;
        let (intent, record, held) = self.read_intent()?;
        verify_prefix(&record["copy_prefix"])?;
        let result = super::combine(
            observe(),
            (|| {
                checkpoint()?;
                held.check()
            })(),
        );
        match result {
            Ok(current) => {
                let classification = classify(&current, &self.after);
                self.result(
                    classification,
                    Some(&current),
                    &intent,
                    0,
                    classification == "after",
                )
            }
            Err(_) => self.result("unknown", None, &intent, 0, false),
        }
    }

    fn intent_binding(&self) -> Result<Value> {
        common::binding(&protected_file(&self.backend, &self.intent)?)
    }

    fn read_intent(&self) -> Result<(Value, Value, Inputs)> {
        let held = Inputs::open([protected_file(&self.backend, &self.intent)?])?;
        let intent_binding = self.intent_binding()?;
        let intent = common::read_bound(&self.backend, &intent_binding, "待核对的原标记意图")?;
        document::exact(
            &intent,
            &[
                "format_version",
                "kind",
                "attempt",
                "controller",
                "binding",
                "copy_prefix",
                "before",
                "after",
            ],
            "Redis 标记恢复意图",
        )?;
        let number = intent["attempt"]
            .as_u64()
            .filter(|number| *number > 0 && *number < self.attempt)
            .ok_or("Redis 标记意图缺少此前的执行阶段")?;
        Stage::verify_intent_controller(&self.backend, &self.run, number, &intent["controller"])?;
        if intent["format_version"] != 1
            || intent["kind"] != "fixture_redis_marker_intent"
            || intent["binding"] != self.binding
            || intent["before"] != empty_image()
            || intent["after"] != self.after
        {
            return Err("原 Redis 标记意图、控制器或完整前后像不同".into());
        }
        held.check()?;
        Ok((intent_binding, intent, held))
    }

    fn result(
        &self,
        classification: &str,
        observed: Option<&Value>,
        intent: &Value,
        write_attempts: u8,
        confirmed: bool,
    ) -> Result<Value> {
        self.check()?;
        let path = self
            .directory
            .join(format!("observation-{:04}.json", self.attempt));
        let observed_sha = observed.map(digest::plan_hash).transpose()?;
        // 未知或混合像只记录摘要及数量，避免将外来的 Redis 值写入公开收据。
        let value = json!({"format_version":1,"kind":"fixture_redis_marker_observation",
            "binding":self.binding,"attempt":self.attempt,"controller":self.controller,
            "intent":intent,"copy_prefix":self.head,"classification":classification,
            "observed":observed.filter(|_| matches!(classification,"before" | "after")),
            "observed_sha256":observed_sha,"namespace_key_count":observed.and_then(|image| image["keys"].as_array()).map(Vec::len),
            "marker_write_attempts":write_attempts,"confirmed":confirmed,
            "remote_writes":if confirmed || write_attempts == 0 { Some(u8::from(write_attempts == 1)) } else { None },
            "clone_verified":false,"restore_qualified":false});
        document::write_new(&path, &value)?;
        self.check()?;
        let read = common::binding(&protected_file(&self.backend, &path)?)?;
        if common::read_bound(&self.backend, &read, "Redis 标记观察收据")? != value {
            return Err("Redis 标记观察收据发布后变化".into());
        }
        Ok(
            json!({"status":if confirmed {"target_cache_markers_confirmed"} else {"needs_reconciliation"},
            "classification":classification,"observation":read,"marker_write_attempts":write_attempts,
            "clone_verified":false,"restore_qualified":false}),
        )
    }
}

fn classify(observed: &Value, after: &Value) -> &'static str {
    if *observed == empty_image() {
        "before"
    } else if observed == after {
        "after"
    } else {
        "mismatch"
    }
}

fn reject_prior_intents(
    root: &std::path::Path,
    current: &std::path::Path,
    allowed: Option<&std::path::Path>,
    verify_directory: impl Fn(&std::path::Path) -> Result<()>,
) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.path() == current {
            continue;
        }
        let file_name = entry.file_name();
        let name = file_name.to_str().ok_or("Redis 标记目录名无效")?;
        if name.len() != 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !entry.file_type()?.is_dir()
        {
            return Err("Redis 标记根目录含未登记代次或链接".into());
        }
        verify_directory(&entry.path())?;
        let intent = entry.path().join("intent.json");
        match fs::symlink_metadata(&intent) {
            Ok(_) if allowed == Some(intent.as_path()) => {}
            Ok(_) => return Err("先前 Redis 代次存在原写入意图，禁止跨代次发送标记写入".into()),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn reboot_marker_does_not_bypass_an_original_intent_in_another_generation() {
    let root = tempfile::tempdir().unwrap();
    let prior = root.path().join("a".repeat(64));
    let current = root.path().join("b".repeat(64));
    fs::create_dir(&prior).unwrap();
    fs::create_dir(&current).unwrap();
    fs::write(current.join("intent.json"), b"current attempt").unwrap();
    reject_prior_intents(root.path(), &current, None, |_| Ok(())).unwrap();
    fs::write(prior.join("intent.json"), b"unreadable previous intent").unwrap();
    assert!(reject_prior_intents(root.path(), &current, None, |_| Ok(())).is_err());
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/copy_transfer/marker_recovery.rs"]
mod tests;
