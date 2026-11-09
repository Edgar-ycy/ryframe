use std::{ffi::OsString, fs, net::SocketAddr, time::Duration};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::recovery::{clone::native::source, frontend_build::tests as fixture};

const ASSET: &[u8] = b"\x00RyFrame fixture\xff";

struct Fixture {
    _directory: tempfile::TempDir,
    options: Options,
}

impl Fixture {
    fn new() -> Self {
        let directory = fixture::fixture();
        let root = directory.path();
        fs::create_dir(root.join("dist/assets")).unwrap();
        fs::write(root.join("dist/assets/中文 空格.js"), ASSET).unwrap();
        let inventory = source::inventory_value(&source::capture(root).unwrap());
        let head = inventory["source"]["snapshot"]["head"].as_str().unwrap();
        fixture::write_receipt(root, head, &inventory);
        let receipt = root.join("dist").join(content::RECEIPT);
        let receipt_sha256 = hex::encode(Sha256::digest(fs::read(&receipt).unwrap()));
        let options = Options {
            frontend: root.to_owned(),
            receipt,
            receipt_sha256,
            expected_head: head.to_owned(),
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
            runtime_directory: root.to_owned(),
            api_port: 18180,
            b0: false,
        };
        Self {
            _directory: directory,
            options,
        }
    }
}

#[test]
fn receipt_whitelist_is_loaded_once_and_rejects_file_or_receipt_drift() {
    let fixture = Fixture::new();
    let options = &fixture.options;
    let site = content::Site::load(options).unwrap();
    assert_eq!(
        site.assets.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            ".vite/manifest.json",
            "assets/中文 空格.js",
            "build-identity.json",
            "index.html"
        ],
    );
    let unexpected = options.frontend.join("dist/extra.js");
    fs::write(&unexpected, "unregistered").unwrap();
    assert!(content::Site::load(options).is_err());
    fs::remove_file(unexpected).unwrap();
    fs::write(options.frontend.join("dist/assets/中文 空格.js"), "changed").unwrap();
    assert_eq!(site.assets["assets/中文 空格.js"].bytes.as_ref(), ASSET);
    assert!(content::Site::load(options).is_err());
    fs::write(options.frontend.join("dist/assets/中文 空格.js"), ASSET).unwrap();
    let raw = fs::read(&options.receipt).unwrap();
    fs::write(&options.receipt, [raw.as_slice(), b"\n"].concat()).unwrap();
    let error = content::Site::load(options).err().unwrap();
    assert!(error.to_string().contains("SHA-256 不同"));
}

#[tokio::test]
async fn loopback_serves_assets_head_and_spa_and_rejects_unsafe_requests() {
    let fixture = Fixture::new();
    let (listener, site) = prepare(&fixture.options).unwrap();
    let address = listener.local_addr().unwrap();
    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
    let server =
        tokio::spawn(async move { axum::serve(listener, http::router(site)).await.unwrap() });
    let dist = fixture.options.frontend.join("dist");
    fs::write(dist.join("assets/中文 空格.js"), "changed on disk").unwrap();
    fs::write(dist.join("new.js"), "unknown on disk").unwrap();

    let asset = "/assets/%E4%B8%AD%E6%96%87%20%E7%A9%BA%E6%A0%BC.js?v=1";
    let (header, body) = request(address, "GET", asset, "application/json").await;
    assert_eq!(status(&header), 200);
    assert_eq!(body, ASSET);
    assert!(header.contains(&format!("\r\ncontent-length: {}", ASSET.len())));
    assert!(header.contains("javascript; charset=utf-8"));
    assert!(header.contains("\r\ncache-control: no-store"));
    assert!(header.contains("\r\nx-content-type-options: nosniff"));
    let (header, body) = request(address, "HEAD", asset, "application/json").await;
    assert_eq!(status(&header), 200);
    assert!(header.contains(&format!("\r\ncontent-length: {}", ASSET.len())));
    assert!(body.is_empty());
    let (header, body) = request(
        address,
        "GET",
        "/post/42",
        "application/json, text/html; q=0.5",
    )
    .await;
    assert_eq!(status(&header), 200);
    assert_eq!(body, b"fixture asset");

    for (target, accept) in [
        ("/.vite/restore-build.json", "text/html"),
        ("/.vite/%72estore-build.json", "text/html"),
        ("/assets/missing.js", "text/html"),
        ("/new.js", "text/html"),
        ("/api/health", "application/json"),
        ("/post/42", "text/html;q=0"),
    ] {
        assert_eq!(
            status(&request(address, "GET", target, accept).await.0),
            404,
            "{target}"
        );
    }
    for target in [
        "/%2e%2e/secret",
        "/a/%2E/b",
        "/a//b",
        "/a/",
        "/a%5Cb",
        "/%00",
        "/%FF",
        "/%GG",
        "/%",
        "//a",
    ] {
        assert_eq!(
            status(&request(address, "GET", target, "text/html").await.0),
            400,
            "{target}"
        );
    }
    let (header, body) = request(address, "POST", "/", "text/html").await;
    assert_eq!(status(&header), 405);
    assert!(header.contains("\r\nallow: get, head"));
    assert!(body.is_empty());
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[test]
fn invalid_bindings_and_drift_are_rejected_before_listening() {
    let fixture = Fixture::new();
    let options = fixture.options;
    let mut args = [
        "--frontend",
        options.frontend.to_str().unwrap(),
        "--receipt",
        options.receipt.to_str().unwrap(),
        "--receipt-sha256",
        &options.receipt_sha256,
        "--expected-head",
        &options.expected_head,
        "--host",
        "127.0.0.1",
        "--port",
        "4173",
        "--runtime-dir",
        options.runtime_directory.to_str().unwrap(),
        "--api-port",
        "18180",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    assert!(Options::parse(&args).is_ok());
    for (index, value) in [
        (2, "--frontend"),
        (5, "short"),
        (9, "0.0.0.0"),
        (11, "0"),
        (11, "65536"),
        (11, "true"),
        (15, "0"),
        (15, "04173"),
        (15, "4173"),
    ] {
        let mut invalid = args.clone();
        invalid[index] = OsString::from(value);
        assert!(Options::parse(&invalid).is_err(), "{index}={value}");
    }
    assert!(Options::parse(&args[..args.len() - 1]).is_err());
    args.push("--unknown".into());
    assert!(Options::parse(&args).is_err());
    args[16] = "--b0".into();
    assert!(Options::parse(&args).is_err());
    args[7] = frontend_build::b0::HEAD.into();
    assert!(Options::parse(&args).unwrap().b0);
    args.push("--b0".into());
    assert!(Options::parse(&args).is_err());

    let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let mut options = options;
    let SocketAddr::V4(address) = occupied.local_addr().unwrap() else {
        unreachable!()
    };
    options.address = address;
    let foreign = options.frontend.join("dist/foreign-receipt.json");
    fs::copy(&options.receipt, &foreign).unwrap();
    let original = std::mem::replace(&mut options.receipt, foreign.clone());
    let error = prepare(&options).err().unwrap();
    assert!(error.to_string().contains("同根 dist"));
    options.receipt = original;
    fs::remove_file(foreign).unwrap();
    fs::write(options.frontend.join("dist/index.html"), "changed").unwrap();
    let error = prepare(&options).err().unwrap();
    assert!(error.to_string().contains("完整产物"));
}

async fn request(
    address: SocketAddr,
    method: &str,
    target: &str,
    accept: &str,
) -> (String, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(format!("{method} {target} HTTP/1.1\r\nHost: {address}\r\nAccept: {accept}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut output = Vec::new();
        stream.read_to_end(&mut output).await.unwrap();
        let end = output.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap();
        (String::from_utf8_lossy(&output[..end]).to_ascii_lowercase(), output[end + 4..].to_vec())
    }).await.expect("本机静态 HTTP 请求超时")
}

fn status(headers: &str) -> u16 {
    headers
        .split_ascii_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}
