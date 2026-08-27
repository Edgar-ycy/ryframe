//! 非生产环境全资源重建命令入口。

#![cfg(feature = "bin-reset")]

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ryframe::crypto::install_crypto_provider()?;
    ryframe::reset::run(std::env::args().skip(1).collect())
        .await
        .map_err(Into::into)
}
