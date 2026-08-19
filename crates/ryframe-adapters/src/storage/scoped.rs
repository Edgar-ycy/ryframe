use std::{path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;

use super::{
    ObjectListPage, ObjectStorage, PrefixDeleteBatch, StorageError, StorageResult,
    validate_object_prefix,
};

const OWNERSHIP_MARKER_NAME: &str = ".ryframe-owner";

/// 在五类共享逻辑桶内强制使用部署 scope 前缀的对象存储边界。
pub struct ScopedObjectStorage {
    inner: Arc<dyn ObjectStorage>,
    scope_id: String,
    prefix: String,
}

impl ScopedObjectStorage {
    /// `scope_id` 必须先经过 `ryframe-config` 的强类型校验。
    pub fn new(inner: Arc<dyn ObjectStorage>, scope_id: &str) -> Self {
        Self {
            inner,
            scope_id: scope_id.to_owned(),
            prefix: format!("{scope_id}/"),
        }
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn ownership_marker_key(&self) -> String {
        format!("{}{}", self.prefix, OWNERSHIP_MARKER_NAME)
    }

    /// 只读校验指定逻辑桶确实由当前 scope 所有。
    pub async fn verify_ownership_marker(&self, bucket: &str) -> StorageResult<()> {
        let marker_key = self.ownership_marker_key();
        let expected = self.marker_value(bucket);
        let actual = self.inner.get(bucket, &marker_key).await.map_err(|error| {
            StorageError::Readiness(format!(
                "scope '{}' 的对象所有权标记不可读（bucket={bucket}）: {error}",
                self.scope_id
            ))
        })?;
        if actual != expected.as_bytes() {
            return Err(StorageError::Readiness(format!(
                "scope '{}' 的对象所有权标记不匹配（bucket={bucket}）",
                self.scope_id
            )));
        }
        Ok(())
    }

    fn physical_key(&self, logical_key: &str) -> StorageResult<String> {
        if logical_key == OWNERSHIP_MARKER_NAME
            || logical_key.starts_with(&self.prefix)
            || logical_key
                .split('/')
                .next()
                .is_some_and(|segment| segment == self.scope_id)
        {
            return Err(StorageError::InvalidLocation(
                "对象键必须是未加 scope 前缀的业务逻辑键".to_owned(),
            ));
        }
        Ok(format!("{}{}", self.prefix, logical_key))
    }

    fn physical_prefix(&self, logical_prefix: &str) -> StorageResult<String> {
        if logical_prefix.is_empty() {
            return Ok(self.prefix.clone());
        }
        validate_object_prefix(logical_prefix)?;
        if logical_prefix.starts_with(&self.prefix)
            || logical_prefix
                .split('/')
                .next()
                .is_some_and(|segment| segment == self.scope_id)
        {
            return Err(StorageError::InvalidLocation(
                "对象前缀必须是未加 scope 前缀的业务逻辑前缀".to_owned(),
            ));
        }
        Ok(format!("{}{}", self.prefix, logical_prefix))
    }

    fn marker_value(&self, bucket: &str) -> String {
        format!("ryframe-owner:v1:{}:object-storage:{bucket}", self.scope_id)
    }

    async fn ensure_ownership_marker(&self, bucket: &str) -> StorageResult<()> {
        let marker_key = self.ownership_marker_key();
        if self.inner.exists(bucket, &marker_key).await? {
            return self.verify_ownership_marker(bucket).await;
        }
        let marker = self.marker_value(bucket);
        self.inner
            .put(bucket, &marker_key, marker.as_bytes(), "text/plain")
            .await?;
        self.verify_ownership_marker(bucket).await
    }
}

#[async_trait]
impl ObjectStorage for ScopedObjectStorage {
    fn late_put_completion_bound(&self) -> Duration {
        self.inner.late_put_completion_bound()
    }

    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> StorageResult<()> {
        self.inner
            .put(bucket, &self.physical_key(key)?, data, content_type)
            .await
    }

    async fn put_file(
        &self,
        bucket: &str,
        key: &str,
        path: &Path,
        content_type: &str,
        sha256_hex: Option<&str>,
    ) -> StorageResult<()> {
        self.inner
            .put_file(
                bucket,
                &self.physical_key(key)?,
                path,
                content_type,
                sha256_hex,
            )
            .await
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        self.inner.get(bucket, &self.physical_key(key)?).await
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
        self.inner.delete(bucket, &self.physical_key(key)?).await
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
        self.inner.exists(bucket, &self.physical_key(key)?).await
    }

    async fn list_page(
        &self,
        bucket: &str,
        prefix: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> StorageResult<ObjectListPage> {
        let physical_prefix = self.physical_prefix(prefix)?;
        let page = self
            .inner
            .list_page(bucket, &physical_prefix, cursor, limit)
            .await?;
        let keys = page
            .keys
            .into_iter()
            .map(|key| {
                key.strip_prefix(&self.prefix)
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        StorageError::InvalidResponse(
                            "scoped storage received an object outside its scope".to_owned(),
                        )
                    })
            })
            .collect::<StorageResult<Vec<_>>>()?;
        Ok(ObjectListPage {
            keys,
            next_cursor: page.next_cursor,
        })
    }

    async fn delete_prefix_batch(
        &self,
        bucket: &str,
        prefix: &str,
        limit: usize,
    ) -> StorageResult<PrefixDeleteBatch> {
        self.inner
            .delete_prefix_batch(bucket, &self.physical_prefix(prefix)?, limit)
            .await
    }

    async fn prefix_is_empty(&self, bucket: &str, prefix: &str) -> StorageResult<bool> {
        self.inner
            .prefix_is_empty(bucket, &self.physical_prefix(prefix)?)
            .await
    }

    async fn ensure_bucket(&self, bucket: &str) -> StorageResult<()> {
        self.inner.ensure_bucket(bucket).await?;
        self.ensure_ownership_marker(bucket).await
    }

    async fn readiness_check(&self, bucket: &str) -> StorageResult<()> {
        self.inner.readiness_check(bucket).await?;
        self.verify_ownership_marker(bucket).await
    }
}

#[cfg(test)]
mod tests {
    use super::ScopedObjectStorage;
    use crate::storage::{LocalObjectStorage, ObjectStorage};
    use std::sync::Arc;

    fn storage() -> ScopedObjectStorage {
        let directory = tempfile::tempdir().expect("创建测试目录");
        let inner = Arc::new(LocalObjectStorage::new(directory.path().to_path_buf()));
        ScopedObjectStorage::new(inner, "test-a")
    }

    #[test]
    fn physical_object_key_is_always_scoped_once() {
        let storage = storage();
        assert_eq!(
            storage
                .physical_key("jobs/result.xlsx")
                .expect("对象键有效"),
            "test-a/jobs/result.xlsx"
        );
        assert!(storage.physical_key("test-a/jobs/result.xlsx").is_err());
        assert!(storage.physical_key(".ryframe-owner").is_err());
    }

    #[test]
    fn physical_prefix_rejects_fuzzy_and_pre_scoped_values() {
        let storage = storage();
        assert_eq!(
            storage.physical_prefix("").expect("scope 根前缀"),
            "test-a/"
        );
        assert_eq!(
            storage.physical_prefix("jobs/").expect("业务目录前缀"),
            "test-a/jobs/"
        );
        assert!(storage.physical_prefix("jobs").is_err());
        assert!(storage.physical_prefix("test-a/jobs/").is_err());
    }

    #[tokio::test]
    async fn root_cleanup_is_confined_to_the_current_scope() {
        let directory = tempfile::tempdir().expect("创建测试目录");
        let inner: Arc<dyn ObjectStorage> =
            Arc::new(LocalObjectStorage::new(directory.path().join("objects")));
        let scope_a = ScopedObjectStorage::new(Arc::clone(&inner), "test-a");
        let scope_b = ScopedObjectStorage::new(inner, "test-b");
        scope_a
            .ensure_bucket("exports")
            .await
            .expect("初始化 scope A");
        scope_b
            .ensure_bucket("exports")
            .await
            .expect("初始化 scope B");
        scope_a
            .put("exports", "jobs/a.xlsx", b"a", "application/octet-stream")
            .await
            .expect("写入 scope A");
        scope_b
            .put("exports", "jobs/b.xlsx", b"b", "application/octet-stream")
            .await
            .expect("写入 scope B");

        loop {
            let batch = scope_a
                .delete_prefix_batch("exports", "", 1)
                .await
                .expect("分批清理 scope A");
            if !batch.may_have_more {
                break;
            }
        }

        assert!(
            scope_a
                .prefix_is_empty("exports", "")
                .await
                .expect("验证 scope A 为空")
        );
        assert!(
            scope_b
                .exists("exports", "jobs/b.xlsx")
                .await
                .expect("检查 scope B 对象")
        );
        scope_b
            .verify_ownership_marker("exports")
            .await
            .expect("scope B 所有权标记必须保留");
        scope_a
            .ensure_bucket("exports")
            .await
            .expect("重建 scope A 所有权标记");
        scope_a
            .verify_ownership_marker("exports")
            .await
            .expect("scope A 所有权标记必须可重新建立");
    }
}
