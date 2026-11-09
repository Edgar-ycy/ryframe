//! 固定复制声明引用的原目标初始化与只读前像证据。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::super::{
    digest, json as document,
    resources::common,
    target_verify::{guard::Inputs, publication},
};
use crate::Result;

pub(super) struct Proofs {
    pub(super) descriptors: Value,
    held: Inputs,
}

impl Proofs {
    pub(super) fn publish(
        backend: &Path,
        root: &Path,
        source: &Value,
        initial: &Value,
        initialized: &Value,
        stopped: &Value,
    ) -> Result<Self> {
        check_stopped(initial, initialized, stopped)?;
        let directory = root.join(format!("target-proof-{}", uuid::Uuid::new_v4().simple()));
        common::ensure_new_directory(backend, &directory, "复制目标前像证据")?;
        let mut descriptors = json!({"source_stopped":source});
        for (field, name, value) in [
            ("target_initialized", "initialized.json", initialized),
            ("target_stopped", "stopped.json", stopped),
        ] {
            let path = directory.join(name);
            publication::publish(&path, value)?;
            let binding = digest::file(&path)?;
            let relative = path.strip_prefix(root)?;
            descriptors[field] = json!({
                "file": digest::path_text(relative, "复制目标前像证据")?.replace('\\', "/"),
                "bytes":binding.bytes,"sha256":binding.sha256,
            });
        }
        Self::open(root, &descriptors, source, initial, initialized)
    }

    pub(super) fn open(
        root: &Path,
        descriptors: &Value,
        source: &Value,
        initial: &Value,
        initialized: &Value,
    ) -> Result<Self> {
        document::exact(
            descriptors,
            &["source_stopped", "target_initialized", "target_stopped"],
            "复制两侧原证据",
        )?;
        if descriptors["source_stopped"] != *source {
            return Err("复制计划不属于源导出的原停止证明".into());
        }
        let initial_path = evidence(root, &descriptors["target_initialized"], "initialized.json")?;
        let stopped_path = evidence(root, &descriptors["target_stopped"], "stopped.json")?;
        let held = Inputs::open([initial_path.clone(), stopped_path.clone()])?;
        evidence(root, &descriptors["target_initialized"], "initialized.json")?;
        evidence(root, &descriptors["target_stopped"], "stopped.json")?;
        if document::read(&initial_path)? != *initialized {
            return Err("原计划初始化证明与当前已发布目标不同".into());
        }
        check_stopped(initial, initialized, &document::read(&stopped_path)?)?;
        held.check()?;
        Ok(Self {
            descriptors: descriptors.clone(),
            held,
        })
    }

    pub(super) fn checkpoint(&self) -> Result<()> {
        self.held.checkpoint()
    }
}

fn evidence(root: &Path, binding: &Value, expected: &str) -> Result<PathBuf> {
    document::exact(binding, &["file", "bytes", "sha256"], "目标前像文件绑定")?;
    let path = digest::safe_file(root, document::text(&binding["file"], "目标前像相对路径")?)?;
    let value = digest::file(&path)?;
    if path.file_name().and_then(|name| name.to_str()) != Some(expected)
        || binding["bytes"].as_u64() != Some(value.bytes)
        || binding["sha256"] != value.sha256
    {
        return Err("目标前像原始文件或摘要发生变化".into());
    }
    Ok(path)
}

fn check_stopped(initial: &Value, initialized: &Value, stopped: &Value) -> Result<()> {
    document::exact(
        stopped,
        &[
            "status",
            "initialized",
            "inventory",
            "remote_writes",
            "target_ready",
            "clone_verified",
            "restore_qualified",
        ],
        "初次目标只读复验",
    )?;
    if stopped["status"] != "fresh_target_reverified"
        || stopped["initialized"] != *initial
        || stopped["inventory"]["observations"] != initialized["inventory"]["observations"]
        || stopped["remote_writes"].as_u64() != Some(0)
        || stopped["target_ready"] != false
        || stopped["clone_verified"] != false
        || stopped["restore_qualified"] != false
    {
        return Err("原目标停止及只读复验收据没有绑定完整初始化前像".into());
    }
    Ok(())
}
