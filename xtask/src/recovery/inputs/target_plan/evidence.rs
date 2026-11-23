//! 正式目标计划保护原参考文件、目标计划及其五项原证据。
use std::{fs, path::Path};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::{
        clone::native::{
            resources::common::device_inode_for,
            target_verify::evidence_protocol::inventory_history,
        },
        inputs::document::{self, Document},
        monitoring::execution::ProtectedInputs,
    },
};

pub(in crate::recovery) const NAMES: [&str; 5] = [
    "backup_receipt",
    "comparison_sources",
    "arm_input",
    "fresh_target_verify",
    "product_plan",
];

pub(in crate::recovery) struct Frame<'a> {
    pub(in crate::recovery) reference: &'a Document,
    pub(in crate::recovery) target: Option<Document>,
    pub(in crate::recovery) evidence: Vec<Document>,
    fresh_parent: (u64, u64),
    held: ProtectedInputs,
}

impl<'a> Frame<'a> {
    pub(in crate::recovery) fn published(
        backend: &Path,
        reference: &'a Document,
        target: Document,
    ) -> Result<Self> {
        let evidence = descriptors(&target.value)
            .into_iter()
            .zip(NAMES)
            .map(|(value, label)| {
                document::bound_file(backend, value, label).map(|(_, document)| document)
            })
            .collect::<Result<Vec<_>>>()?;
        Self::read(backend, reference, Some(target), evidence)
    }

    pub(in crate::recovery) fn explicit(
        backend: &Path,
        reference: &'a Document,
        paths: [&Path; 5],
    ) -> Result<Self> {
        let evidence = paths
            .into_iter()
            .zip(NAMES)
            .map(|(path, label)| document::read(path, backend, label))
            .collect::<Result<Vec<_>>>()?;
        Self::read(backend, reference, None, evidence)
    }

    fn read(
        backend: &Path,
        reference: &'a Document,
        target: Option<Document>,
        evidence: Vec<Document>,
    ) -> Result<Self> {
        validate_local_test_path(&reference.path, backend, LocalTestPathKind::ExistingFile)?;
        let held = ProtectedInputs::open(
            std::iter::once(reference)
                .chain(target.iter())
                .chain(&evidence)
                .map(|file| file.path.clone()),
        )?;
        let fresh_parent = check_fresh_parent(&fresh_evidence(&evidence)?.path)?;
        Ok(Self {
            reference,
            target,
            evidence,
            fresh_parent,
            held,
        })
    }

    pub(in crate::recovery) fn check(&self) -> Result<()> {
        self.held.assert_unchanged()?;
        if check_fresh_parent(&fresh_evidence(&self.evidence)?.path)? != self.fresh_parent {
            return Err("目标 verify 原父目录身份发生变化".into());
        }
        for input in std::iter::once(self.reference)
            .chain(self.target.iter())
            .chain(&self.evidence)
        {
            input.assert_unchanged()?;
        }
        Ok(())
    }

    pub(in crate::recovery) fn check_bindings(&self, value: &Value) -> Result<()> {
        if self.evidence.len() != NAMES.len()
            || descriptors(value)
                .into_iter()
                .zip(&self.evidence)
                .any(|(binding, document)| *binding != document::binding(document))
        {
            return Err("参考目标计划没有保持显式五项证据的原路径和原始字节".into());
        }
        Ok(())
    }

    pub(in crate::recovery) fn verify_fresh_inventory(
        &self,
        backend: &Path,
        request: &Value,
    ) -> Result<()> {
        let verify = fresh_evidence(&self.evidence)?;
        let parent = verify.path.parent().ok_or("目标 verify 缺少父目录")?;
        inventory_history::verify(
            backend,
            parent,
            &json!({
                "initial":verify.value["inventory"],
                "request":request,
                "resumed":false,
                "inventory_directory":parent.join("inventory"),
            }),
        )?;
        Ok(())
    }
}

fn descriptors(value: &Value) -> [&Value; 5] {
    [
        &value["backup_receipt"],
        &value["comparison_sources"],
        &value["arm_input"],
        &value["fresh_target"]["verify"],
        &value["product_plan_file"],
    ]
}

fn fresh_evidence(evidence: &[Document]) -> Result<&Document> {
    let [_, _, _, verify, _] = evidence else {
        return Err("参考目标计划必须绑定五项原证据".into());
    };
    Ok(verify)
}

pub(in crate::recovery) fn check_fresh_parent(verify: &Path) -> Result<(u64, u64)> {
    let parent = verify.parent().ok_or("目标 verify 缺少父目录")?;
    let identity = directory_identity(parent)?;
    for name in ["failure.json", "initialize.lock"] {
        match fs::symlink_metadata(parent.join(name)) {
            Ok(_) => return Err("目标 verify 目录存在失败证据或初始化锁，不能消费残留结果".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if directory_identity(parent)? != identity {
        return Err("目标 verify 原父目录身份在核验期间发生变化".into());
    }
    Ok(identity)
}

pub(in crate::recovery) fn directory_identity(path: &Path) -> Result<(u64, u64)> {
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    device_inode_for(path, &fs::symlink_metadata(path)?)
}
