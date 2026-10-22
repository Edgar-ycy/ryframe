use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use ryframe::healthcheck::probe_local_readiness;

fn serve_once(status: &str) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("应绑定临时健康端口");
    let port = listener.local_addr().expect("应读取临时地址").port();
    listener.set_nonblocking(true).unwrap();
    let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let task = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("未收到健康检查连接：{error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = [0_u8; 128];
        let received = stream.read(&mut request).expect("应读取健康检查请求");
        assert!(String::from_utf8_lossy(&request[..received]).starts_with("GET /readyz "));
        stream
            .write_all(response.as_bytes())
            .expect("应写入健康检查响应");
    });
    (port, task)
}

#[test]
fn accepts_ready_response_without_external_http_client() {
    let (port, task) = serve_once("200 OK");
    probe_local_readiness(port).expect("200 响应应通过健康检查");
    task.join().expect("健康检查服务应正常退出");
}

#[test]
fn rejects_non_ready_response() {
    let (port, task) = serve_once("503 Service Unavailable");
    let error = probe_local_readiness(port).expect_err("非 200 响应必须失败");
    assert!(error.to_string().contains("非 200 状态"));
    task.join().expect("健康检查服务应正常退出");
}

fn directory() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("file-healthcheck-")
        .tempdir_in(root)
        .unwrap()
}

fn check_binary(executable: &str, worker: bool) {
    let directory = directory();
    let config = directory.path().join("selected-config");
    super::configuration_fixture::write_configuration(&config, 1, 1, None);
    let (port, task) = serve_once("200 OK");
    let overlay = if worker {
        format!("[jobs]\nhealth_port = {port}\n")
    } else {
        format!("[app]\nport = {port}\n")
    };
    fs::write(config.join("app.test.toml"), overlay).unwrap();
    let output = Command::new(executable)
        .current_dir(directory.path())
        .args(["--healthcheck", "--config-dir", "selected-config"])
        .env("APP_ENV", "test")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    task.join().unwrap();
    assert!(!directory.path().join("must-not-create-storage").exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn api_healthcheck_uses_selected_file_without_assembling_services() {
    check_binary(env!("CARGO_BIN_EXE_ryframe"), false);
}

#[cfg(feature = "bin-worker")]
#[test]
fn worker_healthcheck_uses_selected_file_without_starting_worker() {
    check_binary(env!("CARGO_BIN_EXE_ryframe-worker"), true);
}

#[test]
fn api_healthcheck_missing_configuration_fails_without_creating_files() {
    let directory = directory();
    let output = Command::new(env!("CARGO_BIN_EXE_ryframe"))
        .current_dir(directory.path())
        .args(["--config-dir", "absent", "--healthcheck"])
        .env("APP_ENV", "test")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("absent"));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn readiness_rejects_zero_port_before_connecting() {
    assert!(
        probe_local_readiness(0)
            .unwrap_err()
            .to_string()
            .contains("必须大于 0")
    );
}
