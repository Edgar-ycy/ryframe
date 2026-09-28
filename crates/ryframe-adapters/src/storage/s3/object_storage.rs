use super::*;

#[async_trait]
impl ObjectStorage for S3ObjectStorage {
    async fn digest(&self, bucket: &str, key: &str) -> StorageResult<crate::storage::ObjectDigest> {
        trace_storage_operation("s3", StorageOperation::Digest, async {
            let url = self.object_url(bucket, key)?;
            let mut response = self
                .send_request(
                    StorageOperation::Digest,
                    self.signed_request(Method::GET, url, "UNSIGNED-PAYLOAD")?,
                )
                .await?;
            if !response.status().is_success() {
                return Err(service_error("校验 S3 对象", response));
            }
            let expected_size = response.content_length();
            let mut digest = Sha256::new();
            let mut bytes = 0_u64;
            while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
                bytes += chunk.len() as u64;
                digest.update(&chunk);
            }
            if expected_size.is_some_and(|size| size != bytes) {
                return Err(StorageError::InvalidResponse(
                    "S3 对象长度与响应不一致".into(),
                ));
            }
            Ok(crate::storage::ObjectDigest {
                bytes,
                sha256: hex::encode(digest.finalize()),
            })
        })
        .await
    }
    fn late_put_completion_bound(&self) -> Duration {
        self.request_timeout
    }

    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> StorageResult<()> {
        trace_storage_operation("s3", StorageOperation::Put, async {
            let url = self.object_url(bucket, key)?;
            let payload_hash = hex::encode(Sha256::digest(data));
            let response = self
                .send_request(
                    StorageOperation::Put,
                    self.signed_request(Method::PUT, url, &payload_hash)?
                        .header("Content-Type", content_type)
                        .body(data.to_vec()),
                )
                .await?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(service_error("upload S3 object", response))
            }
        })
        .await
    }

    async fn put_control(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> StorageResult<()> {
        self.put(bucket, key, data, content_type).await
    }

    async fn put_file(
        &self,
        bucket: &str,
        key: &str,
        path: &Path,
        content_type: &str,
        sha256_hex: Option<&str>,
    ) -> StorageResult<()> {
        trace_storage_operation("s3", StorageOperation::Put, async {
            let url = self.object_url(bucket, key)?;
            let (file, content_length, payload_hash) =
                Self::prepare_upload_file(path, sha256_hex).await?;
            let chunks = stream::try_unfold(file, |mut file| async move {
                let mut chunk = vec![0u8; 64 * 1024];
                let read = file.read(&mut chunk).await?;
                if read == 0 {
                    return Ok(None);
                }
                chunk.truncate(read);
                Ok::<_, std::io::Error>(Some((chunk, file)))
            });
            let response = self
                .send_request(
                    StorageOperation::Put,
                    self.signed_request(Method::PUT, url, &payload_hash)?
                        .header("Content-Type", content_type)
                        .header("Content-Length", content_length)
                        .body(Body::wrap_stream(chunks)),
                )
                .await?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(service_error("upload S3 object", response))
            }
        })
        .await
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        trace_storage_operation("s3", StorageOperation::Get, async {
            let url = self.object_url(bucket, key)?;
            let response = self
                .send_request(
                    StorageOperation::Get,
                    self.signed_request(Method::GET, url, "UNSIGNED-PAYLOAD")?,
                )
                .await?;
            if !response.status().is_success() {
                return Err(service_error("download S3 object", response));
            }
            response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(transport_error)
        })
        .await
    }

    async fn get_bounded(
        &self,
        bucket: &str,
        key: &str,
        max_bytes: usize,
    ) -> StorageResult<Vec<u8>> {
        trace_storage_operation("s3", StorageOperation::Get, async {
            if max_bytes == 0 {
                return Err(StorageError::InvalidLocation(
                    "bounded object read limit must be greater than zero".to_owned(),
                ));
            }
            let url = self.object_url(bucket, key)?;
            let mut response = self
                .send_request(
                    StorageOperation::Get,
                    self.signed_request(Method::GET, url, "UNSIGNED-PAYLOAD")?
                        .header("Range", format!("bytes=0-{max_bytes}")),
                )
                .await?;
            if !response.status().is_success() {
                return Err(service_error("download bounded S3 object", response));
            }
            if response
                .content_length()
                .is_some_and(|length| length > max_bytes as u64)
            {
                return Err(StorageError::InvalidResponse(
                    "object exceeds bounded read limit".to_owned(),
                ));
            }
            let mut data = Vec::with_capacity(
                response
                    .content_length()
                    .unwrap_or_default()
                    .min(max_bytes as u64) as usize,
            );
            while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
                if data.len().saturating_add(chunk.len()) > max_bytes {
                    return Err(StorageError::InvalidResponse(
                        "object exceeds bounded read limit".to_owned(),
                    ));
                }
                data.extend_from_slice(&chunk);
            }
            Ok(data)
        })
        .await
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
        trace_storage_operation("s3", StorageOperation::Delete, async {
            let url = self.object_url(bucket, key)?;
            let payload_hash = empty_payload_hash();
            let response = self
                .send_request(
                    StorageOperation::Delete,
                    self.signed_request(Method::DELETE, url, &payload_hash)?,
                )
                .await?;
            if response.status().is_success() || response.status().as_u16() == 404 {
                Ok(())
            } else {
                Err(service_error("delete S3 object", response))
            }
        })
        .await
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
        trace_storage_operation("s3", StorageOperation::Exists, async {
            let url = self.object_url(bucket, key)?;
            let payload_hash = empty_payload_hash();
            let response = self
                .send_request(
                    StorageOperation::ObjectHead,
                    self.signed_request(Method::HEAD, url, &payload_hash)?,
                )
                .await?;
            match response.status().as_u16() {
                200..=299 => Ok(true),
                404 => Ok(false),
                _ => Err(service_error("check S3 object", response)),
            }
        })
        .await
    }

    async fn list_page(
        &self,
        bucket: &str,
        prefix: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> StorageResult<ObjectListPage> {
        trace_storage_operation("s3", StorageOperation::List, async {
            let url = self.list_url(bucket, prefix, cursor, limit)?;
            let response = self
                .send_request(
                    StorageOperation::List,
                    self.signed_request(Method::GET, url, "UNSIGNED-PAYLOAD")?,
                )
                .await?;
            if !response.status().is_success() {
                return Err(service_error("list S3 objects", response));
            }
            let body = Self::read_bounded_list_response(response).await?;
            parse_list_objects_response(&body, prefix, limit)
        })
        .await
    }

    async fn ensure_bucket(&self, bucket: &str) -> StorageResult<()> {
        trace_storage_operation("s3", StorageOperation::EnsureBucket, async {
            if !self.bucket_exists(bucket).await? {
                self.create_bucket(bucket).await?;
            }
            self.enforce_private_bucket(bucket).await
        })
        .await
    }

    async fn readiness_check(&self, bucket: &str) -> StorageResult<()> {
        trace_storage_operation("s3", StorageOperation::Readiness, async {
            if self.bucket_exists(bucket).await? {
                Ok(())
            } else {
                Err(StorageError::Readiness(format!(
                    "required bucket '{bucket}' does not exist"
                )))
            }
        })
        .await
    }
}
