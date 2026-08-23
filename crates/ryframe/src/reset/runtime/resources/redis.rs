use super::*;

pub struct RedisReset {
    client: RedisClient,
    sentinel_key: String,
    sentinel_value: Vec<u8>,
}

impl RedisReset {
    pub async fn connect_and_inspect(
        config: &RedisConfig,
        sentinel_key: &str,
        manifest: &ResetManifest,
        legacy_exclusive: bool,
    ) -> ResetResult<(Self, PhaseEvidence)> {
        validate_redis_identity(config, manifest)?;
        let resource = manifest
            .redis
            .as_ref()
            .ok_or_else(|| ResetError::new("Redis manifest 缺失"))?;
        if crate::reset::model::sha256_hex(sentinel_key.as_bytes())
            != resource.outside_sentinel_key_sha256
            || sentinel_key.starts_with(&resource.namespace)
        {
            return Err(ResetError::new("Redis scope 外哨兵键与不可变清单不匹配"));
        }
        let client = RedisClient::connect(config).await.map_err(|error| {
            let detail = error
                .to_string()
                .replace(&config.connection_url(), "<Redis 连接地址>")
                .replace(&config.password, "<已隐藏>");
            ResetError::new(format!(
                "Redis 连接预检失败（错误类型：{:?}，详情：{detail}）",
                error.kind(),
            ))
        })?;
        client
            .ping()
            .await
            .map_err(|_| ResetError::new("Redis PING 预检失败"))?;
        let marker = raw_get_bounded(
            client.conn().clone(),
            &resource.ownership_marker_key,
            MAX_MARKER_BYTES,
        )
        .await?;
        match marker {
            Some(value) if value.as_slice() == resource.ownership_marker.as_bytes() => {}
            Some(_) => return Err(ResetError::new("Redis scope 所有权 marker 不匹配")),
            None if legacy_exclusive => {}
            None => {
                return Err(ResetError::new(
                    "Redis namespace 缺少所有权 marker；仅能通过明确的 dev/test 旧资源独占配置接管",
                ));
            }
        }
        let sentinel_value =
            raw_get_bounded(client.conn().clone(), sentinel_key, MAX_SENTINEL_BYTES)
                .await?
                .ok_or_else(|| ResetError::new("Redis scope 外哨兵键不存在"))?;
        Ok((
            Self {
                client,
                sentinel_key: sentinel_key.to_owned(),
                sentinel_value,
            },
            PhaseEvidence::from([
                ("namespace".into(), resource.namespace.clone()),
                ("outside_sentinel".into(), "verified".into()),
            ]),
        ))
    }

    pub async fn prove_capabilities(
        &self,
        manifest: &ResetManifest,
        ledger: &ResetLedger,
        guard: &MysqlReset,
    ) -> ResetResult<()> {
        let resource = manifest
            .redis
            .as_ref()
            .ok_or_else(|| ResetError::new("Redis manifest 缺失"))?;
        guard.assert_locks_held().await?;
        self.verify_sentinel_unchanged().await?;
        self.verify_ownership_before_purge(resource, manifest.legacy_ownership.redis_exclusive)
            .await?;
        if raw_get_bounded(
            self.client.conn().clone(),
            &resource.ownership_marker_key,
            MAX_MARKER_BYTES,
        )
        .await?
        .is_none()
        {
            guard.assert_locks_held().await?;
            self.verify_sentinel_unchanged().await?;
            raw_set(
                self.client.conn().clone(),
                &resource.ownership_marker_key,
                resource.ownership_marker.as_bytes(),
            )
            .await?;
        }
        self.verify_ownership_before_purge(resource, false).await?;
        let probe = reset_probe_key(&resource.namespace, &ledger.plan_hash)?;
        let probe_value = format!("ryframe-reset-probe:v1:{}", ledger.plan_hash);
        match raw_get_bounded(self.client.conn().clone(), &probe, MAX_MARKER_BYTES).await? {
            None => {}
            Some(value) if value.as_slice() == probe_value.as_bytes() => {}
            Some(_) => {
                return Err(ResetError::new(
                    "Redis scoped 能力探针键已存在，拒绝覆盖非本次 reset 数据",
                ));
            }
        }
        guard.assert_locks_held().await?;
        self.verify_sentinel_unchanged().await?;
        if let Err(error) =
            raw_set(self.client.conn().clone(), &probe, probe_value.as_bytes()).await
        {
            self.cleanup_probe(&probe, &resource.namespace, guard)
                .await?;
            return Err(error);
        }
        let keys = match scan_scope_keys(self.client.conn().clone(), &probe, &resource.namespace, 1)
            .await
        {
            Ok(keys) => keys,
            Err(error) => {
                self.cleanup_probe(&probe, &resource.namespace, guard)
                    .await?;
                return Err(error);
            }
        };
        if keys.as_slice() != [probe.as_str()] {
            self.cleanup_probe(&probe, &resource.namespace, guard)
                .await?;
            return Err(ResetError::new("Redis SCAN 未精确返回 scoped 能力探针"));
        }
        guard.assert_locks_held().await?;
        self.verify_sentinel_unchanged().await?;
        let removed =
            match raw_unlink_exact(self.client.conn().clone(), &probe, &resource.namespace).await {
                Ok(removed) => removed,
                Err(error) => {
                    self.cleanup_probe(&probe, &resource.namespace, guard)
                        .await?;
                    return Err(error);
                }
            };
        if removed != 1 {
            self.cleanup_probe(&probe, &resource.namespace, guard)
                .await?;
            return Err(ResetError::new("Redis scoped 能力探针删除数量不匹配"));
        }
        let remaining =
            match raw_get_bounded(self.client.conn().clone(), &probe, MAX_MARKER_BYTES).await {
                Ok(remaining) => remaining,
                Err(error) => {
                    self.cleanup_probe(&probe, &resource.namespace, guard)
                        .await?;
                    return Err(error);
                }
            };
        if remaining.is_some() {
            self.cleanup_probe(&probe, &resource.namespace, guard)
                .await?;
            return Err(ResetError::new("Redis scoped 能力探针删除后仍然存在"));
        }
        self.verify_sentinel_unchanged().await
    }

    pub async fn purge(
        &self,
        manifest: &ResetManifest,
        progress: &mut ResourceProgress<'_>,
        guard: &MysqlReset,
    ) -> ResetResult<PhaseEvidence> {
        let resource = manifest
            .redis
            .as_ref()
            .ok_or_else(|| ResetError::new("Redis manifest 缺失"))?;
        let resource_key = redis_resource_key(resource);
        if progress.is_complete(&resource_key) {
            self.verify(manifest).await?;
            return Ok(PhaseEvidence::from([("deleted_keys".into(), "0".into())]));
        }
        guard.assert_locks_held().await?;
        self.verify_sentinel_unchanged().await?;
        self.verify_ownership_before_purge(resource, false).await?;
        progress.begin(&resource_key)?;
        let pattern = format!("{}*", resource.namespace);
        let mut deleted = 0_u64;
        for _ in 0..4 {
            deleted = deleted
                .saturating_add(scan_and_unlink_scope(self, guard, resource, &pattern).await?);
            if scan_scope_keys(self.client.conn().clone(), &pattern, &resource.namespace, 2)
                .await?
                .as_slice()
                == [resource.ownership_marker_key.as_str()]
            {
                break;
            }
        }
        if scan_scope_keys(self.client.conn().clone(), &pattern, &resource.namespace, 2)
            .await?
            .as_slice()
            != [resource.ownership_marker_key.as_str()]
        {
            return Err(ResetError::new("Redis scope 清理后仍有键"));
        }
        guard.assert_locks_held().await?;
        self.verify_sentinel_unchanged().await?;
        self.verify(manifest).await?;
        progress.complete(&resource_key)?;
        Ok(PhaseEvidence::from([
            ("deleted_keys".into(), deleted.to_string()),
            ("outside_sentinel".into(), "unchanged".into()),
        ]))
    }

    pub async fn verify(&self, manifest: &ResetManifest) -> ResetResult<()> {
        let resource = manifest
            .redis
            .as_ref()
            .ok_or_else(|| ResetError::new("Redis manifest 缺失"))?;
        let keys = scan_scope_keys(
            self.client.conn().clone(),
            &format!("{}*", resource.namespace),
            &resource.namespace,
            2,
        )
        .await?;
        if keys.as_slice() != [resource.ownership_marker_key.as_str()] {
            return Err(ResetError::new("Redis scope 并非仅保留所有权 marker"));
        }
        let marker = raw_get_bounded(
            self.client.conn().clone(),
            &resource.ownership_marker_key,
            MAX_MARKER_BYTES,
        )
        .await?
        .ok_or_else(|| ResetError::new("Redis 所有权 marker 不存在"))?;
        if marker.as_slice() != resource.ownership_marker.as_bytes() {
            return Err(ResetError::new("Redis 所有权 marker 复验失败"));
        }
        self.verify_sentinel_unchanged().await
    }

    async fn verify_sentinel_unchanged(&self) -> ResetResult<()> {
        let sentinel = raw_get_bounded(
            self.client.conn().clone(),
            &self.sentinel_key,
            MAX_SENTINEL_BYTES,
        )
        .await?
        .ok_or_else(|| ResetError::new("Redis scope 外哨兵键在重建后消失"))?;
        if sentinel != self.sentinel_value {
            return Err(ResetError::new("Redis scope 外哨兵值在重建期间发生变化"));
        }
        Ok(())
    }

    async fn verify_ownership_before_purge(
        &self,
        resource: &crate::reset::model::RedisResource,
        legacy_exclusive: bool,
    ) -> ResetResult<()> {
        let marker = raw_get_bounded(
            self.client.conn().clone(),
            &resource.ownership_marker_key,
            MAX_MARKER_BYTES,
        )
        .await?;
        match marker {
            Some(value) if value.as_slice() == resource.ownership_marker.as_bytes() => Ok(()),
            Some(_) => Err(ResetError::new(
                "Redis scope 在清理前的所有权 marker 不匹配",
            )),
            None if legacy_exclusive => Ok(()),
            None => Err(ResetError::new("Redis scope 在清理前缺少所有权 marker")),
        }
    }

    async fn cleanup_probe(
        &self,
        probe: &str,
        namespace: &str,
        guard: &MysqlReset,
    ) -> ResetResult<()> {
        guard.assert_locks_held().await?;
        let cleanup = raw_unlink_exact(self.client.conn().clone(), probe, namespace).await;
        let sentinel = self.verify_sentinel_unchanged().await;
        sentinel?;
        cleanup.map(|_| ())
    }
}

pub fn reset_probe_key(namespace: &str, plan_hash: &str) -> ResetResult<String> {
    if namespace.is_empty()
        || plan_hash.len() != 64
        || !plan_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ResetError::new(
            "Redis 能力探针收到非法 namespace 或 plan hash",
        ));
    }
    Ok(format!("{namespace}.ryframe-reset-probe:{plan_hash}"))
}

async fn scan_and_unlink_scope(
    redis: &RedisReset,
    guard: &MysqlReset,
    resource: &crate::reset::model::RedisResource,
    pattern: &str,
) -> ResetResult<u64> {
    let mut connection = redis.client.conn().clone();
    let mut cursor = 0_u64;
    let mut pages = 0_usize;
    let mut deleted = 0_u64;
    loop {
        guard.assert_locks_held().await?;
        redis.verify_sentinel_unchanged().await?;
        if pages >= MAX_REDIS_SCAN_PAGES {
            return Err(ResetError::new("Redis SCAN 超过安全页数上限"));
        }
        let (next, mut keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(REDIS_SCAN_BATCH_SIZE)
            .query_async(&mut connection)
            .await
            .map_err(|_| ResetError::new("Redis scope SCAN 失败"))?;
        validate_physical_keys(&keys, &resource.namespace)?;
        retain_deletable_redis_keys(&mut keys, &resource.ownership_marker_key);
        if !keys.is_empty() {
            guard.assert_locks_held().await?;
            redis.verify_sentinel_unchanged().await?;
            redis.verify_ownership_before_purge(resource, false).await?;
            let mut command = redis::cmd("UNLINK");
            for key in &keys {
                command.arg(key);
            }
            let count: u64 = command
                .query_async(&mut connection)
                .await
                .map_err(|_| ResetError::new("Redis scope UNLINK 失败"))?;
            deleted = deleted.saturating_add(count);
        }
        pages += 1;
        if next == 0 {
            return Ok(deleted);
        }
        cursor = next;
    }
}

async fn scan_scope_keys(
    mut connection: ConnectionManager,
    pattern: &str,
    namespace: &str,
    maximum: usize,
) -> ResetResult<Vec<String>> {
    let mut cursor = 0_u64;
    let mut pages = 0_usize;
    let mut keys = BTreeSet::new();
    loop {
        if pages >= MAX_REDIS_SCAN_PAGES {
            return Err(ResetError::new("Redis 验证 SCAN 超过安全页数上限"));
        }
        let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(REDIS_SCAN_BATCH_SIZE)
            .query_async(&mut connection)
            .await
            .map_err(|_| ResetError::new("Redis scope 验证 SCAN 失败"))?;
        validate_physical_keys(&batch, namespace)?;
        keys.extend(batch);
        if keys.len() > maximum {
            break;
        }
        pages += 1;
        if next == 0 {
            break;
        }
        cursor = next;
    }
    Ok(keys.into_iter().collect())
}

pub fn validate_physical_keys(keys: &[String], namespace: &str) -> ResetResult<()> {
    if keys.iter().any(|key| !key.starts_with(namespace)) {
        return Err(ResetError::new(
            "Redis SCAN 返回 scope 外键，拒绝执行 UNLINK",
        ));
    }
    Ok(())
}

pub fn is_deletable_object_key(key: &str, ownership_marker_key: &str) -> bool {
    key != ownership_marker_key
}

pub fn retain_deletable_redis_keys(keys: &mut Vec<String>, ownership_marker_key: &str) {
    keys.retain(|key| key != ownership_marker_key);
}

async fn raw_get_bounded(
    mut connection: ConnectionManager,
    key: &str,
    max_bytes: usize,
) -> ResetResult<Option<Vec<u8>>> {
    if max_bytes == 0 || max_bytes > i64::MAX as usize {
        return Err(ResetError::new("Redis 有界读取上限无效"));
    }
    let initial_length: u64 = redis::cmd("STRLEN")
        .arg(key)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis 精确键长度读取失败"))?;
    if initial_length > max_bytes as u64 {
        return Err(ResetError::new("Redis 控制键超过有界读取上限"));
    }
    let value: Vec<u8> = redis::cmd("GETRANGE")
        .arg(key)
        .arg(0)
        .arg(max_bytes as i64)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis 精确键有界读取失败"))?;
    if value.len() > max_bytes {
        return Err(ResetError::new("Redis 控制键在读取期间超过上限"));
    }
    let exists: bool = redis::cmd("EXISTS")
        .arg(key)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis 精确键存在性复验失败"))?;
    if !exists {
        return if value.is_empty() {
            Ok(None)
        } else {
            Err(ResetError::new("Redis 控制键在读取期间发生变化"))
        };
    }
    let final_length: u64 = redis::cmd("STRLEN")
        .arg(key)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis 精确键长度复验失败"))?;
    if final_length > max_bytes as u64
        || final_length != initial_length
        || final_length != value.len() as u64
    {
        return Err(ResetError::new("Redis 控制键在有界读取期间发生变化"));
    }
    Ok(Some(value))
}

async fn raw_set(mut connection: ConnectionManager, key: &str, value: &[u8]) -> ResetResult<()> {
    redis::cmd("SET")
        .arg(key)
        .arg(value)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis scoped 键写入失败"))
}

async fn raw_unlink_exact(
    mut connection: ConnectionManager,
    key: &str,
    namespace: &str,
) -> ResetResult<u64> {
    if !key.starts_with(namespace) {
        return Err(ResetError::new("Redis 精确 UNLINK 键不属于当前 namespace"));
    }
    redis::cmd("UNLINK")
        .arg(key)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis 精确 scoped 键 UNLINK 失败"))
}
