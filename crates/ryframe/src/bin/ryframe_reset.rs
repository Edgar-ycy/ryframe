// Qodana 默认不启用 Cargo 的 required-features；目标仍由清单强制门禁。
//noinspection MissingFeatures
//! 非生产环境全资源重建命令入口。

#![cfg(feature = "destructive-reset")]

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ryframe::reset::run(std::env::args().skip(1).collect())
        .await
        .map_err(Into::into)
}
