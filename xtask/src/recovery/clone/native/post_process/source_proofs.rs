//! 同一复制后阶段内持有导出根及已核验的静态证明文件。
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};

use super::files;
use crate::{
    Result,
    cli::PostCopyOperation,
    recovery::clone::native::{
        json as document, lifecycle::stage::StageRequest, resources::common,
        target_verify::guard::BackupInputs,
    },
};

pub(super) fn for_stage(
    request: &StageRequest<'_>,
    copy_stage: &Value,
    export: &Value,
    proof_files: Value,
    enabled: bool,
) -> Result<(Value, Option<HeldSourceProofs>)> {
    if !enabled {
        return Ok((proof_files, None));
    }
    if request.stage != "post-copy"
        || !PostCopyOperation::supports_held_originals(request.mode)
        || copy_stage != "source_to_seed"
    {
        return Err("持有导出证明必须属于受支持的 source_to_seed 复制后阶段".into());
    }
    let (held, outside) = HeldSourceProofs::open(request.backend, export, proof_files)?;
    Ok((outside, Some(held)))
}

pub(in crate::recovery::clone::native) struct HeldSourceProofs {
    backend: PathBuf,
    export: Value,
    root: PathBuf,
    proofs: Map<String, Value>,
    inputs: BackupInputs,
}

impl HeldSourceProofs {
    pub(in crate::recovery::clone::native) fn open(
        backend: &Path,
        export: &Value,
        proof_files: Value,
    ) -> Result<(Self, Value)> {
        let export_path = common::bound_file(backend, export, "原复制导出")?;
        let root = export_path.parent().ok_or("原复制导出缺少证据根")?;
        let published = common::read_bound(backend, export, "原复制导出")?;
        if export_path != root.join("export.json")
            || Path::new(document::text(&published["artifact_root"], "原导出证据根")?) != root
        {
            return Err("原复制导出收据与原证据根不一致".into());
        }
        let original_request = common::bound_file(backend, &published["request"], "原导出请求")?;
        let Value::Object(proofs) = proof_files else {
            return Err("原来源证明集合必须为对象".into());
        };
        if !proofs
            .keys()
            .any(|name| Path::new(name) == original_request.as_path())
        {
            return Err("原来源证明集合缺少原导出请求".into());
        }
        require_unfailed(root)?;

        let mut directories = BTreeSet::new();
        let mut paths = BTreeSet::from([export_path.clone()]);
        let mut outside = Map::new();
        for (name, digest) in &proofs {
            let path = common::common_file(backend, Path::new(name), "原来源证明文件")?;
            if path.starts_with(root) {
                directories.extend(
                    path.ancestors()
                        .skip(1)
                        .take_while(|parent| *parent != root)
                        .map(Path::to_path_buf),
                );
                paths.insert(path);
            } else if path == original_request {
                outside.insert(name.clone(), digest.clone());
            } else {
                return Err("导出根之外的证明文件只能是已登记的原导出请求".into());
            }
        }
        let inputs = BackupInputs::open(root, directories, paths)?;
        let held = Self {
            backend: backend.to_owned(),
            export: export.clone(),
            root: root.to_owned(),
            proofs,
            inputs,
        };
        held.check_end()?;
        Ok((held, Value::Object(outside)))
    }

    pub(in crate::recovery::clone::native) fn checkpoint(&self) -> Result<()> {
        self.inputs.checkpoint()?;
        require_unfailed(&self.root)
    }

    pub(in crate::recovery::clone::native) fn check_end(&self) -> Result<()> {
        require_unfailed(&self.root)?;
        self.inputs.check_identities()?;
        common::bound_file(&self.backend, &self.export, "原复制导出")?;
        files::check_proof_files(&self.proofs, "持有期间原来源证明文件字节或摘要变化")?;
        common::bound_file(&self.backend, &self.export, "原复制导出")?;
        self.inputs.check_identities()?;
        require_unfailed(&self.root)
    }
}

fn require_unfailed(root: &Path) -> Result<()> {
    if root.join("failure.json").try_exists()? {
        return Err("源证据包含失败标记，不能使用残留成功文件".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/post_process/source_proofs.rs"]
mod tests;
