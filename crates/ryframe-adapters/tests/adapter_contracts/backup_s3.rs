use ryframe_adapters::storage::{ObjectStorage, S3Config, S3ObjectStorage, StorageError};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

fn object_server(content: Vec<u8>, declared: usize) -> (S3ObjectStorage, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut header = Vec::new();
        let mut byte = [0_u8];
        while !header.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
            assert!(header.len() < 16_384);
        }
        assert!(header.starts_with(b"GET /exports/scope/data.bin "));
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        for chunk in content.chunks(8192) {
            stream.write_all(chunk).unwrap();
        }
    });
    let storage = S3ObjectStorage::new(S3Config {
        endpoint: address.to_string(),
        access_key: "fixture".into(),
        secret_key: "fixture-secret".into(),
        use_ssl: false,
        root_ca_pem: None,
        region: "us-east-1".into(),
        request_timeout_secs: 5,
    })
    .unwrap();
    (storage, server)
}

#[tokio::test]
async fn streaming_digest_reads_all_chunks_and_rejects_truncated_response() {
    let bytes = vec![7_u8; 1024 * 1024 + 17];
    let expected = hex::encode(Sha256::digest(&bytes));
    let (storage, server) = object_server(bytes.clone(), bytes.len());
    let digest = storage.digest("exports", "scope/data.bin").await.unwrap();
    server.join().unwrap();
    assert_eq!(digest.bytes, bytes.len() as u64);
    assert_eq!(digest.sha256, expected);
    let (storage, server) = object_server(b"short".to_vec(), 100);
    assert!(storage.digest("exports", "scope/data.bin").await.is_err());
    server.join().unwrap();
}

#[tokio::test]
async fn streaming_digest_rejects_redirects_without_following_them() {
    let redirect = TcpListener::bind("127.0.0.1:0").unwrap();
    let redirect_address = redirect.local_addr().unwrap();
    let destination = TcpListener::bind("127.0.0.1:0").unwrap();
    let destination_address = destination.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = redirect.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut header = Vec::new();
        let mut byte = [0_u8];
        while !header.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
            assert!(header.len() < 16_384);
        }
        write!(
            stream,
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{destination_address}/exports/scope/data.bin\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
    });
    let storage = S3ObjectStorage::new(S3Config {
        endpoint: redirect_address.to_string(),
        access_key: "fixture".into(),
        secret_key: "fixture-secret".into(),
        use_ssl: false,
        root_ca_pem: None,
        region: "us-east-1".into(),
        request_timeout_secs: 5,
    })
    .unwrap();
    let error = storage
        .digest("exports", "scope/data.bin")
        .await
        .unwrap_err();
    server.join().unwrap();
    assert!(matches!(error, StorageError::Service { status: 307, .. }));
    destination.set_nonblocking(true).unwrap();
    assert!(matches!(
        destination.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
}
