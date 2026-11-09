//! 静态权威读取期间持有文件身份和字节；不创建锁文件或报告。
use crate::{
    Result,
    recovery::{
        inputs::document::{self, Document},
        monitoring::execution::ProtectedInputs,
    },
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(in crate::recovery::runtime) struct Inputs {
    backend: PathBuf,
    documents: BTreeMap<PathBuf, Document>,
    guards: Vec<ProtectedInputs>,
}

impl Inputs {
    pub(in crate::recovery::runtime) fn new(backend: &Path) -> Self {
        Self {
            backend: backend.to_owned(),
            documents: BTreeMap::new(),
            guards: Vec::new(),
        }
    }

    pub(in crate::recovery::runtime) fn protect(&mut self, path: &Path) -> Result<()> {
        self.guards.push(ProtectedInputs::open([path.to_owned()])?);
        Ok(())
    }

    pub(in crate::recovery::runtime) fn read(&mut self, path: &Path) -> Result<Document> {
        if let Some(document) = self.documents.get(path) {
            document.assert_unchanged()?;
            return Ok(document.clone());
        }
        self.protect(path)?;
        let document = if path.starts_with(self.backend.join(".local-tests")) {
            document::read(path, &self.backend, "恢复静态权威输入")?
        } else {
            crate::local_test_path::validate_external_existing_file(path)?;
            let value = crate::recovery::clone::native::json::read(path)?;
            let raw = std::fs::read(path)?;
            let parsed = crate::recovery::clone::native::json::parse_fragment(
                std::str::from_utf8(&raw).map_err(|_| "恢复静态输入必须为 UTF-8")?,
                "恢复静态输入",
            )?;
            if parsed != value {
                return Err("恢复静态输入读取期间漂移".into());
            }
            Document {
                path: path.to_owned(),
                raw,
                value,
            }
        };
        self.documents.insert(path.to_owned(), document.clone());
        Ok(document)
    }

    pub(in crate::recovery::runtime) fn bound(&mut self, descriptor: &Value) -> Result<Document> {
        let binding = document::binding_shape(descriptor, "恢复静态权威绑定")?;
        crate::local_test_path::validate_local_test_path(
            &binding.path,
            &self.backend,
            crate::local_test_path::LocalTestPathKind::ExistingFile,
        )?;
        let document = self.read(&binding.path)?;
        if document::binding(&document) != *descriptor {
            return Err("恢复静态权威输入与登记描述不一致".into());
        }
        Ok(document)
    }

    pub(in crate::recovery::runtime) fn finish<T>(&self, result: Result<T>) -> Result<T> {
        let checked: Result<()> = (|| {
            for document in self.documents.values() {
                document.assert_unchanged()?;
            }
            for guard in &self.guards {
                guard.assert_unchanged()?;
            }
            Ok(())
        })();
        match (result, checked) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(after)) => Err(format!("{error}；完整输入后验失败：{after}").into()),
        }
    }
}
