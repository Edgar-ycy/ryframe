//! 在独立进程中验证产品 S3 客户端不消费宿主代理配置。
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use ryframe_adapters::storage::{S3Config, S3ObjectStorage};

const PROBE: &str = "storage_file_configuration::file_configuration_probe";
const COMPLETED: &str = "S3_FILE_CONFIGURATION_PROBE_COMPLETED";

#[test]
fn s3_uses_file_endpoint_without_inherited_proxy_routing() {
    let origin = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    origin.set_nonblocking(true).unwrap();
    proxy.set_nonblocking(true).unwrap();
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("s3.toml"),
        format!(
            "backend='s3'\nendpoint='http://{}'\naccess_key='file-access'\n\
             secret_key='file-secret'\nregion='us-east-1'\nrequest_timeout_secs=2\n",
            origin.local_addr().unwrap(),
        ),
    )
    .unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            PROBE,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .current_dir(directory.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("APP_ENV", "test")
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .env_remove("REQUEST_METHOD");
    for key in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env(key, format!("http://{}", proxy.local_addr().unwrap()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = ProbeChild(command.spawn().unwrap());
    let (status, origin_requests, proxy_requests) = observe_routing(&mut child.0, &origin, &proxy);
    let mut output = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .take(64 * 1024)
        .read_to_string(&mut output)
        .unwrap();
    let mut diagnostic = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .take(64 * 1024)
        .read_to_string(&mut diagnostic)
        .unwrap();
    assert!(
        status.is_some_and(|status| status.success()) || origin_requests + proxy_requests > 0,
        "S3 子进程未进入网络观察：{status:?}；{output}；{diagnostic}"
    );
    assert_eq!(
        proxy_requests, 0,
        "宿主代理收到了产品 S3 请求；配置端点请求数={origin_requests}"
    );
    assert_eq!(origin_requests, 1, "必须实际访问文件中的 S3 端点");
    assert!(
        status.is_some_and(|status| status.success()),
        "S3 子进程失败或超时：{status:?}"
    );
    assert!(output.contains(COMPLETED), "子进程未实际完成对象存储调用");
}

fn observe_routing(
    child: &mut Child,
    origin: &TcpListener,
    proxy: &TcpListener,
) -> (Option<ExitStatus>, usize, usize) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut origin_requests = 0;
    let mut proxy_requests = 0;
    let status = loop {
        for (listener, count, response) in [
            (origin, &mut origin_requests, "HTTP/1.1 200 OK"),
            (proxy, &mut proxy_requests, "HTTP/1.1 502 Bad Gateway"),
        ] {
            match listener.accept() {
                Ok((stream, _)) => {
                    *count += 1;
                    answer(stream, response);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("本机 S3 观察失败：{error}"),
            }
        }
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        thread::sleep(Duration::from_millis(5));
    };
    (status, origin_requests, proxy_requests)
}

fn answer(mut stream: TcpStream, status: &str) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = Vec::new();
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let mut chunk = [0; 1024];
        let length = stream.read(&mut chunk).unwrap();
        assert!(
            length > 0 && request.len() + length <= 32 * 1024,
            "S3 请求头不完整或超限"
        );
        request.extend_from_slice(&chunk[..length]);
    }
    stream
        .write_all(format!("{status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())
        .unwrap();
}

struct ProbeChild(Child);

impl Drop for ProbeChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[tokio::test]
#[ignore = "由文件配置测试用独立环境和临时目录启动"]
async fn file_configuration_probe() {
    let configuration: ryframe_config::ObjectStorageConfig =
        toml::from_str(&fs::read_to_string("s3.toml").unwrap()).unwrap();
    let storage = S3ObjectStorage::new(S3Config {
        endpoint: configuration.endpoint,
        access_key: configuration.access_key,
        secret_key: configuration.secret_key,
        use_ssl: configuration.use_ssl,
        root_ca_pem: None,
        region: configuration.region,
        request_timeout_secs: configuration.request_timeout_secs,
    })
    .unwrap();
    assert!(storage.bucket_exists("exports").await.unwrap());
    println!("{COMPLETED}");
}
