use crate::storage::{ObjectListPage, ObjectStorage};
use ryframe_application::{ports::backup::*, system::operations::BACKUP_OBJECT_BUCKETS};
use ryframe_kernel::{AppError, AppResult};
use std::{collections::BTreeSet, sync::Arc};

pub fn object_verifier(
    storage: Arc<dyn ObjectStorage>,
    endpoint: String,
    scope_id: String,
) -> Arc<dyn BackupObjectVerifier> {
    Arc::new(BackupObjects {
        storage,
        endpoint,
        scope_id,
    })
}

struct BackupObjects {
    storage: Arc<dyn ObjectStorage>,
    endpoint: String,
    scope_id: String,
}

#[async_trait::async_trait]
impl BackupObjectVerifier for BackupObjects {
    async fn validate_restore_targets(&self, plan: &RestorePlan) -> AppResult<()> {
        if plan.object_endpoint != self.endpoint
            || plan.scope_id != self.scope_id
            || plan.object_prefix != format!("{}/", self.scope_id)
        {
            return Err(AppError::Validation(
                "恢复对象目标与登记的配置不一致".into(),
            ));
        }
        for bucket in BACKUP_OBJECT_BUCKETS {
            self.verify_owner(bucket).await?;
        }
        Ok(())
    }

    async fn snapshot(&self) -> AppResult<Vec<ObjectBackup>> {
        let mut result = Vec::new();
        for bucket in BACKUP_OBJECT_BUCKETS {
            result.push(self.bucket_snapshot(bucket).await?);
        }
        Ok(result)
    }

    async fn restored_objects(
        &self,
        manifest: &BackupManifest,
        plan: &RestorePlan,
    ) -> AppResult<()> {
        self.validate_restore_targets(plan).await?;
        let expected_buckets = manifest
            .objects
            .iter()
            .map(|item| item.bucket.as_str())
            .collect::<BTreeSet<_>>();
        if manifest.objects.len() != BACKUP_OBJECT_BUCKETS.len()
            || expected_buckets.len() != manifest.objects.len()
            || expected_buckets != BACKUP_OBJECT_BUCKETS.iter().copied().collect()
        {
            return Err(AppError::Validation(
                "备份对象清单没有覆盖全部业务桶".into(),
            ));
        }
        for expected in &manifest.objects {
            let actual = self.bucket_snapshot(&expected.bucket).await?;
            let mut rewritten = expected.entries.clone();
            for entry in &mut rewritten {
                crate::storage::validate_object_key(&entry.key)
                    .map_err(|_| AppError::Validation("备份对象键无效".into()))?;
                let key = entry
                    .key
                    .strip_prefix(&expected.prefix)
                    .ok_or_else(|| AppError::Validation("备份对象越过 source scope".into()))?;
                entry.key = format!("{}{key}", plan.object_prefix);
                crate::storage::validate_object_key(&entry.key)
                    .map_err(|_| AppError::Validation("恢复对象键无效".into()))?;
            }
            rewritten.sort_by(|left, right| left.key.cmp(&right.key));
            if rewritten != actual.entries {
                return Err(AppError::Validation(
                    "恢复对象缺失、多余或内容校验失败".into(),
                ));
            }
        }
        Ok(())
    }
}

impl BackupObjects {
    async fn verify_owner(&self, bucket: &str) -> AppResult<()> {
        let marker_key = format!("{}/.ryframe-owner", self.scope_id);
        let marker = self
            .storage
            .get_bounded(bucket, &marker_key, 1024)
            .await
            .map_err(|_| AppError::Validation("备份对象 ownership 不可读".into()))?;
        if marker
            != format!("ryframe-owner:v1:{}:object-storage:{bucket}", self.scope_id).as_bytes()
        {
            return Err(AppError::Validation("备份对象 ownership 不匹配".into()));
        }
        Ok(())
    }

    async fn bucket_snapshot(&self, bucket: &str) -> AppResult<ObjectBackup> {
        let prefix = format!("{}/", self.scope_id);
        let marker_key = format!("{prefix}.ryframe-owner");
        self.verify_owner(bucket).await?;
        let keys = self.keys(bucket, &prefix).await?;
        let mut entries = Vec::with_capacity(keys.len());
        for key in keys {
            if key == marker_key {
                continue;
            }
            let digest = self
                .storage
                .digest(bucket, &key)
                .await
                .map_err(|_| AppError::Validation("备份对象完整内容校验失败".into()))?;
            entries.push(BackupObjectDigest {
                key,
                bytes: digest.bytes,
                sha256: digest.sha256,
            });
        }
        Ok(ObjectBackup {
            bucket: bucket.into(),
            prefix,
            entries,
        })
    }

    async fn keys(&self, bucket: &str, prefix: &str) -> AppResult<BTreeSet<String>> {
        let mut cursor = None;
        let mut cursors = BTreeSet::new();
        let mut keys = BTreeSet::new();
        loop {
            let page = self
                .storage
                .list_page(bucket, prefix, cursor.as_deref(), 1000)
                .await
                .map_err(|_| AppError::Validation("无法列举已登记对象范围".into()))?;
            cursor = accept_page(prefix, &mut keys, &mut cursors, page)?;
            if cursor.is_none() {
                return Ok(keys);
            }
        }
    }
}

fn accept_page(
    prefix: &str,
    keys: &mut BTreeSet<String>,
    cursors: &mut BTreeSet<String>,
    page: ObjectListPage,
) -> AppResult<Option<String>> {
    if page.keys.len() > 1_000 {
        return Err(AppError::Validation("对象列举单页超过请求上限".into()));
    }
    let before = keys.len();
    for key in page.keys {
        if !key.starts_with(prefix)
            || crate::storage::validate_object_key(&key).is_err()
            || !keys.insert(key)
            || keys.len() > 100_001
        {
            return Err(AppError::Validation(
                "对象列举越界、重复或超过备份清单上限".into(),
            ));
        }
    }
    let Some(cursor) = page.next_cursor else {
        return Ok(None);
    };
    if keys.len() == before
        || cursor.is_empty()
        || cursor.len() > 4_096
        || cursor.chars().any(char::is_control)
    {
        return Err(AppError::Validation(
            "对象列举未取得进展或返回无效游标".into(),
        ));
    }
    if !cursors.insert(cursor.clone()) || cursors.len() > 10_000 {
        return Err(AppError::Validation("对象列举游标循环或页数过多".into()));
    }
    Ok(Some(cursor))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(keys: &[&str], cursor: Option<&str>) -> ObjectListPage {
        ObjectListPage {
            keys: keys.iter().map(|key| (*key).to_owned()).collect(),
            next_cursor: cursor.map(str::to_owned),
        }
    }

    #[test]
    fn page_validation_rejects_escaped_and_duplicate_keys() {
        for invalid in ["other/data", "scope/../data", "scope/data\\part", "scope/"] {
            assert!(
                accept_page(
                    "scope/",
                    &mut BTreeSet::new(),
                    &mut BTreeSet::new(),
                    page(&[invalid], None),
                )
                .is_err()
            );
        }
        assert!(
            accept_page(
                "scope/",
                &mut BTreeSet::new(),
                &mut BTreeSet::new(),
                page(&["scope/data", "scope/data"], None),
            )
            .is_err()
        );
    }

    #[test]
    fn page_validation_rejects_cursor_loops_and_pages_without_progress() {
        let mut keys = BTreeSet::new();
        let mut cursors = BTreeSet::new();
        assert_eq!(
            accept_page(
                "scope/",
                &mut keys,
                &mut cursors,
                page(&["scope/a"], Some("cursor-a")),
            )
            .unwrap(),
            Some("cursor-a".into())
        );
        assert!(
            accept_page(
                "scope/",
                &mut keys,
                &mut cursors,
                page(&["scope/b"], Some("cursor-a")),
            )
            .is_err()
        );
        assert!(
            accept_page(
                "scope/",
                &mut BTreeSet::new(),
                &mut BTreeSet::new(),
                page(&[], Some("cursor-b")),
            )
            .is_err()
        );
    }
}
