//! Windows 复制计划只复用同一窗口内原对象的字节证明。
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
    root: PathBuf,
    #[cfg(windows)]
    objects: Value,
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
        let (_, declaration) = bound_file(
            backend,
            &json!({"path":path,"bytes":wrapper["input_file"]["bytes"],
                "sha256":wrapper["input_file"]["sha256"]}),
            "持有计划原输入",
        )?;
        let root = input::artifact_root(&declaration.value["artifact_root"], backend)?;
        let objects = declaration.value["objects"].clone();
        let artifacts = declared_artifacts(&objects)?;
        let mut directories = BTreeSet::new();
        let mut files = Vec::with_capacity(artifacts.len());
        for name in artifacts.keys() {
            let file = digest::safe_file(&root, name)?;
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
            root,
            objects,
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
        super::verify_plan_digest(&self.plan, &self.plan_digest)
    }
}

impl HeldPlanObjects {
    pub(super) fn require_declaration(&self, root: &Path, objects: &Value) -> Result<()> {
        #[cfg(windows)]
        {
            if root != self.root || objects != &self.objects {
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
