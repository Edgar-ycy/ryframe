use ryframe_config::RedisConfig;

pub(super) fn redis_timeout_error(message: &'static str) -> redis::RedisError {
    redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::TimedOut, message))
}

pub(super) async fn build_client(config: &RedisConfig) -> Result<redis::Client, redis::RedisError> {
    let url = config.connection_url();
    if !config.tls {
        return redis::Client::open(url);
    }

    let root_cert = read_optional_pem(config.tls_ca.as_deref()).await?;
    let client_tls = match (
        config.tls_client_cert.as_deref(),
        config.tls_client_key.as_deref(),
    ) {
        (Some(cert), Some(key)) => Some(redis::ClientTlsConfig {
            client_cert: read_pem(cert).await?,
            client_key: read_pem(key).await?,
        }),
        _ => None,
    };
    redis::Client::build_with_tls(
        url,
        redis::TlsCertificates {
            client_tls,
            root_cert,
        },
    )
}

async fn read_optional_pem(path: Option<&str>) -> Result<Option<Vec<u8>>, redis::RedisError> {
    match path.filter(|path| !path.trim().is_empty()) {
        Some(path) => read_pem(path).await.map(Some),
        None => Ok(None),
    }
}

async fn read_pem(path: &str) -> Result<Vec<u8>, redis::RedisError> {
    tokio::fs::read(path).await.map_err(|error| {
        redis::RedisError::from(std::io::Error::new(
            error.kind(),
            format!("unable to read Redis TLS file {path}: {error}"),
        ))
    })
}
