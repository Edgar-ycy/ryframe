use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use reqwest::{Method, RequestBuilder, Response, Url};
use sha2::{Digest, Sha256};

use super::{
    ObjectStorage, StorageError, StorageResult, key_segments, public_object_url,
    signing::SigV4Signer, validate_bucket,
};

/// Connection and signing settings for a path-style S3-compatible endpoint.
#[derive(Clone, Debug)]
pub struct S3Config {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub use_ssl: bool,
    pub region: String,
    /// Optional CDN or explicitly public object base URL.
    pub public_base_url: Option<String>,
}

/// S3-compatible HTTP backend suitable for AWS S3 and MinIO.
pub struct S3ObjectStorage {
    endpoint: Url,
    access_key: String,
    secret_key: String,
    region: String,
    public_base_url: Option<String>,
    client: reqwest::Client,
}

impl S3ObjectStorage {
    pub fn new(config: S3Config) -> StorageResult<Self> {
        if config.access_key.trim().is_empty() || config.secret_key.is_empty() {
            return Err(StorageError::Configuration(
                "S3 access_key and secret_key are required".to_owned(),
            ));
        }
        if config.region.is_empty()
            || !config
                .region
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(StorageError::Configuration(
                "S3 region must contain only letters, digits, or hyphens".to_owned(),
            ));
        }

        let endpoint = normalize_endpoint(&config.endpoint, config.use_ssl)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            endpoint,
            access_key: config.access_key,
            secret_key: config.secret_key,
            region: config.region,
            public_base_url: config
                .public_base_url
                .filter(|base_url| !base_url.trim().is_empty())
                .map(|base_url| base_url.trim_end_matches('/').to_owned()),
            client,
        })
    }

    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    pub async fn bucket_exists(&self, bucket: &str) -> StorageResult<bool> {
        let url = self.bucket_url(bucket)?;
        let payload_hash = empty_payload_hash();
        let response = self
            .signed_request(Method::HEAD, url, &payload_hash)?
            .send()
            .await?;
        match response.status().as_u16() {
            200..=299 => Ok(true),
            404 => Ok(false),
            _ => Err(service_error("check S3 bucket", response).await),
        }
    }

    pub async fn create_bucket(&self, bucket: &str) -> StorageResult<()> {
        let url = self.bucket_url(bucket)?;
        let body = if self.region == "us-east-1" {
            String::new()
        } else {
            format!(
                "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><LocationConstraint>{}</LocationConstraint></CreateBucketConfiguration>",
                self.region
            )
        };
        let payload_hash = hex::encode(Sha256::digest(body.as_bytes()));
        let mut request = self.signed_request(Method::PUT, url, &payload_hash)?;
        if !body.is_empty() {
            request = request.header("Content-Type", "application/xml").body(body);
        }
        let response = request.send().await?;
        if response.status().is_success() {
            return Ok(());
        }
        if response.status().as_u16() == 409 && self.bucket_exists(bucket).await? {
            return Ok(());
        }
        Err(service_error("create S3 bucket", response).await)
    }

    fn bucket_url(&self, bucket: &str) -> StorageResult<Url> {
        validate_bucket(bucket)?;
        self.location_url(bucket, None)
    }

    fn object_url(&self, bucket: &str, key: &str) -> StorageResult<Url> {
        validate_bucket(bucket)?;
        self.location_url(bucket, Some(key_segments(key)?))
    }

    fn location_url(&self, bucket: &str, key: Option<Vec<&str>>) -> StorageResult<Url> {
        let mut url = self.endpoint.clone();
        let mut path = url.path_segments_mut().map_err(|_| {
            StorageError::Configuration("S3 endpoint cannot be a base URL".to_owned())
        })?;
        path.pop_if_empty().push(bucket);
        if let Some(segments) = key {
            path.extend(segments);
        }
        drop(path);
        Ok(url)
    }

    fn signed_request(
        &self,
        method: Method,
        url: Url,
        payload_hash: &str,
    ) -> StorageResult<RequestBuilder> {
        let amz_date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let authorization = SigV4Signer {
            access_key: &self.access_key,
            secret_key: &self.secret_key,
            region: &self.region,
        }
        .authorization(method.as_str(), &url, payload_hash, &amz_date)?;

        Ok(self
            .client
            .request(method, url)
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", amz_date)
            .header("Authorization", authorization))
    }
}

#[async_trait]
impl ObjectStorage for S3ObjectStorage {
    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> StorageResult<()> {
        let url = self.object_url(bucket, key)?;
        let payload_hash = hex::encode(Sha256::digest(data));
        let response = self
            .signed_request(Method::PUT, url, &payload_hash)?
            .header("Content-Type", content_type)
            .body(data.to_vec())
            .send()
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(service_error("upload S3 object", response).await)
        }
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        let url = self.object_url(bucket, key)?;
        let response = self
            .signed_request(Method::GET, url, "UNSIGNED-PAYLOAD")?
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(service_error("download S3 object", response).await);
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(StorageError::from)
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
        let url = self.object_url(bucket, key)?;
        let payload_hash = empty_payload_hash();
        let response = self
            .signed_request(Method::DELETE, url, &payload_hash)?
            .send()
            .await?;
        if response.status().is_success() || response.status().as_u16() == 404 {
            Ok(())
        } else {
            Err(service_error("delete S3 object", response).await)
        }
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
        let url = self.object_url(bucket, key)?;
        let payload_hash = empty_payload_hash();
        let response = self
            .signed_request(Method::HEAD, url, &payload_hash)?
            .send()
            .await?;
        match response.status().as_u16() {
            200..=299 => Ok(true),
            404 => Ok(false),
            _ => Err(service_error("check S3 object", response).await),
        }
    }

    fn public_url(&self, bucket: &str, key: &str) -> StorageResult<Option<String>> {
        validate_bucket(bucket)?;
        key_segments(key)?;
        self.public_base_url
            .as_deref()
            .map(|base_url| public_object_url(base_url, bucket, key))
            .transpose()
    }

    async fn ensure_bucket(&self, bucket: &str) -> StorageResult<()> {
        if !self.bucket_exists(bucket).await? {
            self.create_bucket(bucket).await?;
        }
        Ok(())
    }
}

fn normalize_endpoint(endpoint: &str, use_ssl: bool) -> StorageResult<Url> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(StorageError::Configuration(
            "S3 endpoint is required".to_owned(),
        ));
    }
    let scheme = if use_ssl { "https" } else { "http" };
    let raw = if endpoint.contains("://") {
        endpoint.to_owned()
    } else {
        format!("{scheme}://{endpoint}")
    };
    let mut url = Url::parse(&raw)
        .map_err(|error| StorageError::Configuration(format!("invalid S3 endpoint: {error}")))?;
    url.set_scheme(scheme).map_err(|_| {
        StorageError::Configuration("S3 endpoint scheme must be HTTP or HTTPS".to_owned())
    })?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(StorageError::Configuration(
            "S3 endpoint must contain only scheme, host, and optional port".to_owned(),
        ));
    }
    url.set_path("");
    Ok(url)
}

fn empty_payload_hash() -> String {
    hex::encode(Sha256::digest([]))
}

async fn service_error(operation: &'static str, response: Response) -> StorageError {
    let status = response.status().as_u16();
    let mut message = response.text().await.unwrap_or_default();
    message.truncate(2048);
    StorageError::Service {
        operation,
        status,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> S3Config {
        S3Config {
            endpoint: "localhost:9000".to_owned(),
            access_key: "access".to_owned(),
            secret_key: "secret".to_owned(),
            use_ssl: false,
            region: "eu-west-1".to_owned(),
            public_base_url: Some("https://cdn.example.com/files".to_owned()),
        }
    }

    #[test]
    fn endpoint_and_object_urls_are_normalized_and_encoded() {
        let storage = S3ObjectStorage::new(config()).unwrap();

        assert_eq!(storage.endpoint(), "http://localhost:9000/");
        assert_eq!(
            storage
                .object_url("photos", "夏季/photo one.jpg")
                .unwrap()
                .as_str(),
            "http://localhost:9000/photos/%E5%A4%8F%E5%AD%A3/photo%20one.jpg"
        );
        assert_eq!(
            storage
                .public_url("photos", "夏季/photo one.jpg")
                .unwrap()
                .as_deref(),
            Some("https://cdn.example.com/files/photos/%E5%A4%8F%E5%AD%A3/photo%20one.jpg")
        );
    }

    #[test]
    fn endpoint_rejects_credentials_paths_and_missing_secrets() {
        for endpoint in ["http://user@example.com", "http://example.com/path"] {
            let mut invalid = config();
            invalid.endpoint = endpoint.to_owned();
            assert!(S3ObjectStorage::new(invalid).is_err());
        }

        let mut invalid = config();
        invalid.secret_key.clear();
        assert!(S3ObjectStorage::new(invalid).is_err());
    }
}
