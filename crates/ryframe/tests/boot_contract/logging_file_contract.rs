use std::{
    fs,
    io::{self, Read},
    net::TcpListener,
    path::Path,
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use super::configuration_fixture::write_configuration;

const PASSWORD: &str = "logging-file-password-sentinel";
const SECRETS: [&str; 3] = [
    PASSWORD,
    "fixture-database-password",
    "isolated-file-configuration-fixture-secret",
];
type Reader = JoinHandle<io::Result<Vec<u8>>>;

#[test]
fn file_error_level_ignores_rust_log_trace() {
    assert_file_level("error", "trace", false);
}

#[test]
fn file_trace_level_ignores_rust_log_off() {
    assert_file_level("trace", "off", true);
}

fn assert_file_level(level: &str, rejected_override: &str, expects_info: bool) {
    let parent =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests/logging-file-contract");
    fs::create_dir_all(&parent).expect("创建隔离日志测试父目录失败");
    let directory = tempfile::Builder::new()
        .prefix("api-")
        .tempdir_in(&parent)
        .expect("创建隔离日志测试目录失败");
    // 三个监听器都由当前测试独占；数据库端口始终不接受连接或发送握手。
    let database = TcpListener::bind("127.0.0.1:0").expect("占用隔离数据库端口失败");
    let api = TcpListener::bind("127.0.0.1:0").expect("占用隔离 API 端口失败");
    let health = TcpListener::bind("127.0.0.1:0").expect("占用隔离健康端口失败");
    let config = directory.path().join("config");
    write_configuration(
        &config,
        api.local_addr().unwrap().port(),
        health.local_addr().unwrap().port(),
        None,
    );
    write_overlay(
        &config,
        directory.path(),
        database.local_addr().unwrap().port(),
        level,
    );
    let output = run_api(directory.path(), &config, rejected_override)
        .unwrap_or_else(|error| panic!("隔离 API 日志回归执行失败：{error}"));
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        SECRETS.iter().all(|secret| !text.contains(secret)),
        "API 输出泄露测试凭据；输出已隐藏"
    );
    assert!(
        !output.status.success()
            && text.contains("阶段=连接池建立")
            && text.contains("字段=database.primary"),
        "应在数据库握手阶段失败，实际状态={}，输出={text}",
        output.status
    );
    assert_eq!(
        text.contains("configuration loaded"),
        expects_info,
        "文件日志级别未生效，状态={}，输出={text}",
        output.status
    );
    assert!(
        !directory.path().join("storage").exists(),
        "数据库失败前不应初始化对象存储"
    );
    drop((database, api, health));
    directory.close().expect("清理隔离日志测试目录失败");
}

fn write_overlay(config: &Path, root: &Path, database_port: u16, level: &str) {
    let storage = serde_json::to_string(&root.join("storage").to_string_lossy()).unwrap();
    let locales = serde_json::to_string(&root.join("missing-locales").to_string_lossy()).unwrap();
    let overlay = format!(
        r#"[app]
locales_dir = {locales}
[database.primary]
port = {database_port}
password = "{PASSWORD}"
connect_timeout_secs = 1
acquire_timeout_secs = 1
[api_docs]
enabled = false
[telemetry]
enabled = false
[logger]
level = "{level}"
format = "text"
output = "stdout"
[object_storage]
local_base_dir = {storage}
"#
    );
    fs::write(config.join("app.test.toml"), overlay).expect("写入隔离文件配置失败");
}

fn run_api(root: &Path, config: &Path, rejected_override: &str) -> Result<Output, String> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ryframe"));
    command
        .arg("--config-dir")
        .arg(config)
        .current_dir(root)
        .env("APP_ENV", "test")
        .env("RUST_LOG", rejected_override)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let child = command
        .spawn()
        .map_err(|error| format!("启动 API：{error}"))?;
    let mut process = ApiProcess {
        child,
        readers: Vec::new(),
        reaped: false,
    };
    let stdout = process.child.stdout.take().ok_or("缺少 API stdout 管道")?;
    process.readers.push(start_reader(stdout, "stdout")?);
    let stderr = process.child.stderr.take().ok_or("缺少 API stderr 管道")?;
    process.readers.push(start_reader(stderr, "stderr")?);
    process.finish(deadline)
}

fn start_reader(mut stream: impl Read + Send + 'static, name: &str) -> Result<Reader, String> {
    thread::Builder::new()
        .name(format!("logging-contract-{name}"))
        .spawn(move || {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .map_err(|error| format!("启动 {name} 收集线程：{error}"))
}

struct ApiProcess {
    child: Child,
    readers: Vec<Reader>,
    reaped: bool,
}

impl ApiProcess {
    fn finish(&mut self, deadline: Instant) -> Result<Output, String> {
        let status = self.wait_until(deadline);
        let mut errors = Vec::new();
        if let Err(error) = &status {
            errors.push(error.clone());
        }
        if !self.reaped {
            errors.extend(self.stop());
        }
        let streams = self.collect(&mut errors);
        if !errors.is_empty() {
            let captured = streams
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes))
                .collect::<Vec<_>>()
                .join("\n");
            let safe = SECRETS.iter().fold(captured, |text, secret| {
                text.replace(secret, "[已隐藏凭据]")
            });
            return Err(format!("{}；输出={safe}", errors.join("；")));
        }
        let mut streams = streams.into_iter();
        Ok(Output {
            status: status?,
            stdout: streams.next().ok_or("stdout 收集结果缺失")?,
            stderr: streams.next().ok_or("stderr 收集结果缺失")?,
        })
    }

    fn wait_until(&mut self, deadline: Instant) -> Result<ExitStatus, String> {
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.reaped = true;
                    return Ok(status);
                }
                Ok(None) => {}
                Err(error) => return Err(format!("查询 API 退出状态：{error}")),
            }
            if Instant::now() >= deadline {
                return Err("API 超过 10 秒上限，执行精确进程终止与回收".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        // 只操作持有的 Child，不按名称或端口终止其他进程。
        if let Err(error) = self.child.kill() {
            errors.push(format!("终止隔离 API：{error}"));
        }
        match self.child.wait() {
            Ok(_) => self.reaped = true,
            Err(error) => errors.push(format!("回收隔离 API：{error}")),
        }
        errors
    }

    fn collect(&mut self, errors: &mut Vec<String>) -> Vec<Vec<u8>> {
        let mut streams = Vec::new();
        for (index, reader) in self.readers.drain(..).enumerate() {
            match reader.join() {
                Ok(Ok(bytes)) => streams.push(bytes),
                Ok(Err(error)) => errors.push(format!("收集 API 管道 {index}：{error}")),
                Err(_) => errors.push(format!("API 管道 {index} 收集线程异常退出")),
            }
        }
        streams
    }
}

impl Drop for ApiProcess {
    fn drop(&mut self) {
        let mut errors = if self.reaped { Vec::new() } else { self.stop() };
        self.collect(&mut errors);
        if !errors.is_empty() {
            eprintln!("隔离 API 清理失败：{}", errors.join("；"));
        }
    }
}
