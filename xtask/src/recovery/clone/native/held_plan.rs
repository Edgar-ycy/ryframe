//! Windows 持有复制原件，并在同一窗口内复用已完整核验的数据库计划。
use std::path::Path;

#[cfg(windows)]
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use serde_json::Value;
#[cfg(windows)]
use serde_json::json;

use super::input::ArtifactBinding;
#[cfg(windows)]
use super::{digest, input, json as document, target_verify::guard::BackupInputs};
use crate::Result;
#[cfg(windows)]
use crate::{
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::inputs::document::bound_file,
};

pub(super) struct HeldPlanObjects {
    #[cfg(windows)]
    backend: PathBuf,
    #[cfg(windows)]
    plan: PathBuf,
    #[cfg(windows)]
    plan_digest: digest::FileDigest,
    #[cfg(windows)]
    wrapper: Value,
    #[cfg(windows)]
    input_path: PathBuf,
    #[cfg(windows)]
    input_digest: digest::FileDigest,
    #[cfg(windows)]
    declaration: Value,
    #[cfg(windows)]
    root: PathBuf,
    #[cfg(windows)]
    artifacts: BTreeMap<String, digest::FileDigest>,
    #[cfg(windows)]
    inputs: BackupInputs,
    #[cfg(not(windows))]
    impossible: std::convert::Infallible,
}

#[cfg(windows)]
impl HeldPlanObjects {
    pub(in crate::recovery::clone::native) fn open(
        backend: &Path,
        plan: &Path,
        wrapper: &Value,
    ) -> Result<Self> {
        validate_local_test_path(plan, backend, LocalTestPathKind::ExistingFile)?;
        let plan_digest = super::verify_published_plan(plan, wrapper)?;
        let path = Path::new(document::text(&wrapper["input_path"], "计划输入文件")?);
        let (binding, declaration) = bound_file(
            backend,
            &json!({"path":path,"bytes":wrapper["input_file"]["bytes"],
                "sha256":wrapper["input_file"]["sha256"]}),
            "持有计划原输入",
        )?;
        let root = input::artifact_root(&declaration.value["artifact_root"], backend)?;
        let artifacts = declared_artifacts(&declaration.value["objects"])?;
        let mut names = artifacts.keys().cloned().collect::<BTreeSet<_>>();
        for dump in document::array(&declaration.value["databases"], "数据库转储")? {
            names.insert(input::artifact_descriptor(&dump["artifact"])?.file);
        }
        let mut directories = BTreeSet::new();
        let mut files = Vec::with_capacity(names.len());
        for name in names {
            let file = digest::safe_file(&root, &name)?;
            directories.extend(
                file.ancestors()
                    .skip(1)
                    .take_while(|parent| *parent != root.as_path())
                    .map(Path::to_path_buf),
            );
            files.push(file);
        }
        let inputs = BackupInputs::open(&root, directories, files)?;
        let window = Self {
            backend: backend.to_path_buf(),
            plan: plan.to_path_buf(),
            plan_digest,
            wrapper: wrapper.clone(),
            input_path: binding.path,
            input_digest: digest::FileDigest {
                bytes: binding.bytes,
                sha256: binding.sha256,
            },
            declaration: declaration.value,
            root,
            artifacts,
            inputs,
        };
        window.check_end()?;
        Ok(window)
    }

    pub(in crate::recovery::clone::native) fn issue(&self) -> Result<Value> {
        self.verify(true)
    }

    pub(in crate::recovery::clone::native) fn issue_bound(
        &self,
        backend: &Path,
        plan: &Path,
        wrapper: &Value,
    ) -> Result<Value> {
        if backend != self.backend || plan != self.plan || wrapper != &self.wrapper {
            return Err("当前复制计划与持有窗口的原计划不一致".into());
        }
        self.issue()
    }

    pub(in crate::recovery::clone::native) fn check_end(&self) -> Result<()> {
        self.verify(false).map(|_| ())
    }

    fn verify(&self, reuse_held_objects: bool) -> Result<Value> {
        self.checkpoint()?;
        let result = super::issue_with_objects(
            &self.backend,
            &self.plan,
            &self.wrapper,
            reuse_held_objects.then_some(self),
        );
        let checked = self.checkpoint();
        result.and_then(|value| checked.map(|()| value))
    }

    fn checkpoint(&self) -> Result<()> {
        self.inputs.checkpoint()?;
        self.verify_documents()
    }

    fn verify_documents(&self) -> Result<()> {
        super::verify_plan_digest(&self.plan, &self.plan_digest)?;
        if digest::file(&self.input_path)? != self.input_digest {
            return Err("持有窗口内原复制输入的字节发生变化".into());
        }
        Ok(())
    }
}

impl HeldPlanObjects {
    pub(super) fn reuse_databases(
        &self,
        declaration: &Value,
        root: &Path,
        catalog_sha256: &str,
    ) -> Result<(Vec<Value>, Vec<Value>)> {
        #[cfg(windows)]
        {
            if root != self.root || declaration != &self.declaration {
                return Err("持有的数据库计划与当前原件根或完整复制输入不同".into());
            }
            let plan = &self.wrapper["plan"];
            if plan["catalog_sha256"].as_str() != Some(catalog_sha256) {
                return Err("持有窗口的控制库或租户库规范目录摘要变化".into());
            }
            self.verify_documents()?;
            // 打开窗口时已按原入口完整重算并核对这份计划，SQL 字节仍在 rebind_all 中复验。
            Ok((
                document::array(&plan["databases"], "已核验数据库计划")?.to_vec(),
                document::array(&plan["pending_target_actions"], "已核验目标调度待办")?.to_vec(),
            ))
        }
        #[cfg(not(windows))]
        {
            let _ = (declaration, root, catalog_sha256);
            match self.impossible {}
        }
    }

    pub(super) fn require_declaration(&self, root: &Path, objects: &Value) -> Result<()> {
        #[cfg(windows)]
        {
            if root != self.root || objects != &self.declaration["objects"] {
                return Err("持有的原对象根或完整对象声明与复制输入不同".into());
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = (root, objects);
            match self.impossible {}
        }
    }

    pub(super) fn bound_object(&self, root: &Path, value: &Value) -> Result<ArtifactBinding> {
        #[cfg(windows)]
        {
            let artifact = input::artifact_descriptor(value)?;
            if root != self.root || self.artifacts.get(&artifact.file) != Some(&artifact.digest) {
                return Err("原对象证据不属于当前持有范围或摘要不同".into());
            }
            Ok(artifact)
        }
        #[cfg(not(windows))]
        {
            let _ = (root, value);
            match self.impossible {}
        }
    }
}

#[cfg(windows)]
fn declared_artifacts(objects: &Value) -> Result<BTreeMap<String, digest::FileDigest>> {
    let mut artifacts = BTreeMap::new();
    for bucket in document::array(objects, "复制对象桶")? {
        for entry in document::array(&bucket["entries"], "复制对象条目")? {
            let artifact = input::artifact_descriptor(&entry["artifact"])?;
            if let Some(previous) = artifacts.insert(artifact.file, artifact.digest.clone())
                && previous != artifact.digest
            {
                return Err("同一路径的原对象证据声明了不同的字节".into());
            }
        }
    }
    Ok(artifacts)
}

#[cfg(all(test, windows))]
mod tests;
