use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use crate::Result;

#[path = "tls/configuration.rs"]
pub(crate) mod configuration;
use super::integration_configuration;
use integration_configuration::IntegrationConfiguration;

#[path = "tls/certificate.rs"]
mod certificate;
#[path = "tls/fixture.rs"]
mod fixture;
#[path = "tls/process.rs"]
mod process;

#[allow(unused_imports)]
pub(crate) use certificate::generate_certificates_with_program;
#[allow(unused_imports)]
pub(crate) use fixture::FixtureServers;
#[allow(unused_imports)]
pub(crate) use process::{ProcessOutcome, run_redirected};

const LOOPBACK_NAMES: &[&str] = &["127.0.0.1", "::1", "localhost"];
const MAX_LOG_TAIL_LINES: usize = 80;

#[derive(Debug, Clone, Copy)]
pub(crate) struct TestSpec {
    pub(crate) name: &'static str,
    target: &'static str,
    test_name: &'static str,
    features: Option<&'static str>,
}

pub(crate) const TESTS: &[TestSpec] = &[
    TestSpec {
        name: "redis-tls",
        target: "redis_real_protocol",
        test_name: "tls_connection_round_trip_uses_real_redis",
        features: Some("redis-api"),
    },
    TestSpec {
        name: "s3-https",
        target: "s3_https_real",
        test_name: "s3_client_reaches_a_real_https_endpoint",
        features: None,
    },
    TestSpec {
        name: "otlp-https",
        target: "otel_https_real",
        test_name: "otlp_exporter_flushes_over_a_real_https_endpoint",
        features: Some("otel"),
    },
];

#[derive(Debug, Clone)]
pub(crate) struct RedisEndpoint {
    pub(crate) host: String,
    pub(crate) port: u16,
}

struct GateOptions<'a> {
    backend_root: PathBuf,
    target_dir: PathBuf,
    jobs: usize,
    timeout: Duration,
    input: &'a IntegrationConfiguration,
}

// 主控制器加载一次；TLS只消费同一配置对象并按捕获前像复核，不重新加载参数。
pub(crate) fn run_from_configuration(
    backend_root: &Path,
    target_dir: &Path,
    configuration: &IntegrationConfiguration,
) -> Result<PathBuf> {
    configuration.verify()?;
    run_gate(GateOptions {
        backend_root: backend_root.to_owned(),
        target_dir: target_dir.to_owned(),
        jobs: configuration.test_jobs,
        timeout: configuration.timeout,
        input: configuration,
    })
}

fn run_gate(options: GateOptions<'_>) -> Result<PathBuf> {
    options.input.verify()?;
    integration_configuration::files::create_parents(&options.input.artifact_root)?;
    let output = reserve_output_dir(&options.input.artifact_root, &options.input.run_id)?;
    println!("tls_integration_artifacts={}", output.display());
    let material = output.join("fixture-material");
    let mut failures = Vec::new();
    let mut fixtures = None;
    let mut published = configuration::Published::new();
    if let Err(error) = start_and_run(&options, &output, &material, &mut fixtures, &mut published) {
        failures.push(error.to_string());
    }
    if let Some(mut servers) = fixtures
        && let Err(error) = servers.stop()
    {
        failures.push(error.to_string());
    }
    let cleanup = published.clean(options.input);
    let preserve_material = !cleanup.is_empty();
    failures.extend(cleanup);
    if !preserve_material {
        if let Err(error) = remove_material(&output, &material) {
            failures.push(error.to_string());
        }
    } else {
        failures.push(format!(
            "配置发布/归属未确认，保留证书目录 {}，请核对现场，不自动重放",
            material.display()
        ));
    }
    if let Err(error) = options.input.verify() {
        failures.push(error);
    }
    write_summary(&output, &failures)?;
    if failures.is_empty() {
        Ok(output)
    } else {
        print_failure_tails(&output);
        Err(failures.join("\n").into())
    }
}

fn start_and_run(
    options: &GateOptions<'_>,
    output: &Path,
    material_dir: &Path,
    fixtures: &mut Option<FixtureServers>,
    published: &mut configuration::Published,
) -> Result<()> {
    let redis = options
        .input
        .redis
        .app
        .redis
        .as_ref()
        .expect("输入已验证redis");
    let endpoint = redis_endpoint(&redis.host, &redis.port.to_string())?;
    let upstream = check_redis(
        &endpoint,
        &redis.password,
        Duration::from_secs(redis.timeout_secs),
    )?;
    fs::create_dir(material_dir).map_err(|error| {
        integration_configuration::files::io_error(material_dir, "创建TLS证书目录", error)
    })?;
    let material = certificate::generate_certificates_with_program(
        options.input.openssl_program.clone().into_os_string(),
        material_dir,
        &output.join("openssl.log"),
    )?;
    *fixtures = Some(FixtureServers::start(
        &material.server_cert,
        &material.server_key,
        upstream,
        &output.join("fixtures.log"),
    )?);
    let servers = fixtures.as_ref().expect("已保存fixture句柄");
    published.publish(
        &options.backend_root,
        options.input,
        &material.ca_cert,
        servers.redis_port(),
        servers.https_port(),
    )?;
    let failures = collect_test_failures(|spec| {
        if let Err(error) = published.verify(options.input) {
            return Some(error);
        }
        let failure = run_test(spec, options, &output.join(format!("{}.log", spec.name)));
        match (failure, published.verify(options.input)) {
            (Some(error), Err(check)) => Some(format!("{error}；{check}")),
            (failure, Ok(())) => failure,
            (None, Err(error)) => Some(error),
        }
    });
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n").into())
    }
}

pub(crate) fn test_command(spec: &TestSpec, target_dir: &Path, jobs: usize) -> Vec<String> {
    let mut command = vec![
        "cargo".to_owned(),
        "test".to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        target_dir.to_string_lossy().into_owned(),
        "-p".to_owned(),
        "ryframe-adapters".to_owned(),
    ];
    if let Some(features) = spec.features {
        command.extend(["--features".to_owned(), features.to_owned()]);
    }
    command.extend([
        "--test".to_owned(),
        spec.target.to_owned(),
        "--jobs".to_owned(),
        jobs.max(1).to_string(),
        spec.test_name.to_owned(),
        "--".to_owned(),
        "--exact".to_owned(),
        "--nocapture".to_owned(),
    ]);
    command
}

fn run_test(spec: &TestSpec, options: &GateOptions<'_>, log_path: &Path) -> Option<String> {
    let mut log = match OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(log_path)
    {
        Ok(log) => log,
        Err(error) => {
            return Some(format!("无法创建测试日志 {}：{error}", log_path.display()));
        }
    };
    let arguments = test_command(spec, &options.target_dir, options.jobs);
    let mut command = Command::new(&arguments[0]);
    command
        .args(&arguments[1..])
        .current_dir(&options.backend_root);
    command.env("APP_ENV", "test");
    match process::run_redirected(command, &mut log, options.timeout) {
        Ok(process::ProcessOutcome::Exited(status)) if status.success() => None,
        Ok(process::ProcessOutcome::Exited(status)) => Some(format!(
            "退出码 {}（{}）",
            status
                .code()
                .map_or_else(|| status.to_string(), |code| code.to_string()),
            log_path.display()
        )),
        Ok(process::ProcessOutcome::TimedOut) => Some(format!(
            "超时（{} 秒，{}）",
            options.timeout.as_secs(),
            log_path.display()
        )),
        Err(error) => Some(format!("执行失败：{error}（{}）", log_path.display())),
    }
}

pub(crate) fn collect_test_failures(
    mut run: impl FnMut(&TestSpec) -> Option<String>,
) -> Vec<String> {
    TESTS
        .iter()
        .filter_map(|spec| run(spec).map(|error| format!("{}: {error}", spec.name)))
        .collect()
}

pub(crate) fn redis_endpoint(host: &str, raw_port: &str) -> Result<RedisEndpoint> {
    let host = host.trim();
    if !LOOPBACK_NAMES.contains(&host) {
        return Err("TLS 门禁只允许连接回环 Redis，拒绝接触共享远程服务".into());
    }
    let port = raw_port
        .trim()
        .parse::<u16>()
        .map_err(|error| format!("redis.port 不是有效端口：{error}；请填写1..65535整数"))?;
    if port == 0 {
        return Err("redis.port 必须在 1 到 65535 之间；请修正文件配置".into());
    }
    Ok(RedisEndpoint {
        host: host.to_owned(),
        port,
    })
}

pub(crate) fn check_redis(
    endpoint: &RedisEndpoint,
    password: &str,
    timeout: Duration,
) -> Result<SocketAddr> {
    let operation = if password.is_empty() { "PING" } else { "AUTH" };
    let addresses = fixture::resolved_endpoints(endpoint).map_err(|_| {
        format!(
            "Redis {operation} 端点={}:{}；IOkind=不可用；端点解析失败；请核对回环主机和端口",
            endpoint.host, endpoint.port
        )
    })?;
    let mut errors = Vec::new();
    for address in addresses {
        if !address.ip().is_loopback() {
            return Err(probe_error(
                address,
                operation,
                None,
                "解析结果不是回环地址，拒绝连接",
            ));
        }
        match probe_redis(address, password, timeout) {
            Ok(()) => return Ok(address),
            Err(error) => errors.push(error.to_string()),
        }
    }
    Err(errors.join("；").into())
}

fn probe_redis(address: SocketAddr, password: &str, timeout: Duration) -> Result<()> {
    let operation = if password.is_empty() { "PING" } else { "AUTH" };
    if timeout.is_zero() {
        return Err(probe_error(
            address,
            operation,
            None,
            "探测timeout必须大于0",
        ));
    }
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| probe_error(address, operation, None, "探测timeout超出可表示范围"))?;
    let mut connection = TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| probe_error(address, operation, Some(error.kind()), "连接失败"))?;
    if !password.is_empty() {
        // RESP bulk长度按UTF8字节计算，密码中的空格或CR/LF不会形成额外命令。
        let auth = format!("*2\r\n$4\r\nAUTH\r\n${}\r\n{password}\r\n", password.len());
        probe_exchange(
            &mut connection,
            address,
            "AUTH",
            auth.as_bytes(),
            b"+OK\r\n",
            deadline,
        )?;
    }
    probe_exchange(
        &mut connection,
        address,
        "PING",
        b"*1\r\n$4\r\nPING\r\n",
        b"+PONG\r\n",
        deadline,
    )
}

fn probe_exchange(
    connection: &mut TcpStream,
    address: SocketAddr,
    operation: &str,
    request: &[u8],
    expected: &[u8],
    deadline: Instant,
) -> Result<()> {
    let sent = (|| -> std::io::Result<()> {
        connection.set_write_timeout(Some(probe_remaining(deadline)?))?;
        connection.write_all(request)?;
        connection.flush()
    })();
    sent.map_err(|error| probe_error(address, operation, Some(error.kind()), "写入命令失败"))?;
    let response = read_probe_line(connection, deadline).map_err(|error| {
        probe_error(address, operation, Some(error.kind()), "读取完整RESP行失败")
    })?;
    if response != expected {
        return Err(probe_error(
            address,
            operation,
            None,
            "未返回预期完整RESP成功行；认证拒绝或协议不符",
        ));
    }
    Ok(())
}

fn read_probe_line(connection: &mut TcpStream, deadline: Instant) -> std::io::Result<Vec<u8>> {
    let mut response = Vec::with_capacity(16);
    for _ in 0..1024 {
        connection.set_read_timeout(Some(probe_remaining(deadline)?))?;
        let mut byte = [0_u8; 1];
        connection.read_exact(&mut byte)?;
        response.push(byte[0]);
        if response.ends_with(b"\r\n") {
            return Ok(response);
        }
        if byte[0] == b'\n' {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
    }
    Err(std::io::ErrorKind::InvalidData.into())
}

fn probe_remaining(deadline: Instant) -> std::io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| std::io::ErrorKind::TimedOut.into())
}

fn probe_error(
    address: SocketAddr,
    operation: &str,
    kind: Option<std::io::ErrorKind>,
    reason: &str,
) -> Box<dyn std::error::Error> {
    format!("Redis {operation} 端点={address}；IOkind={kind:?}；原因={reason}；请核对文件中的Redis密码、端点和timeout及服务协议，不自动降级或忽略认证").into()
}

pub(crate) fn reserve_output_dir(root: &Path, run_id: &str) -> Result<PathBuf> {
    let safe_run_id = sanitize_run_id(run_id);
    fs::create_dir_all(root)
        .map_err(|error| format!("无法创建 TLS 日志根目录 {}：{error}", root.display()))?;
    for sequence in 0..100 {
        let suffix = if sequence == 0 {
            String::new()
        } else {
            format!("-{sequence}")
        };
        let candidate = root.join(format!("{safe_run_id}{suffix}"));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(
                    format!("无法创建 TLS 日志目录 {}：{error}", candidate.display()).into(),
                );
            }
        }
    }
    Err(format!("无法在 {} 创建不覆盖历史日志的运行目录", root.display()).into())
}

fn sanitize_run_id(run_id: &str) -> String {
    let mut result = String::new();
    let mut previous_replacement = false;
    for character in run_id.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-') {
            result.push(character);
            previous_replacement = false;
        } else if !previous_replacement {
            result.push('-');
            previous_replacement = true;
        }
    }
    let result = result.trim_matches(|character| matches!(character, '-' | '.'));
    if result.is_empty() {
        "local".to_owned()
    } else {
        result.to_owned()
    }
}

fn remove_material(output_dir: &Path, material_dir: &Path) -> Result<()> {
    if !material_dir.exists() {
        return Ok(());
    }
    let output = output_dir
        .canonicalize()
        .map_err(|error| format!("无法规范化 TLS 运行目录 {}：{error}", output_dir.display()))?;
    let material = material_dir.canonicalize().map_err(|error| {
        format!(
            "无法规范化临时 TLS 证书目录 {}：{error}",
            material_dir.display()
        )
    })?;
    if material.parent() != Some(output.as_path())
        || material.file_name().and_then(|name| name.to_str()) != Some("fixture-material")
    {
        return Err(format!("拒绝清理 TLS 运行目录边界外的路径：{}", material.display()).into());
    }
    fs::remove_dir_all(&material).map_err(|error| {
        format!(
            "清理临时 TLS 证书目录失败（{}）：{error}",
            material.display()
        )
        .into()
    })
}

fn write_summary(output_dir: &Path, failures: &[String]) -> Result<()> {
    let summary = if failures.is_empty() {
        "三个 AWS-LC TLS 出站测试全部通过".to_owned()
    } else {
        failures.join("\n")
    };
    fs::write(output_dir.join("summary.txt"), format!("{summary}\n"))
        .map_err(|error| format!("无法写入 TLS 门禁摘要：{error}"))?;
    Ok(())
}

fn print_failure_tails(output_dir: &Path) {
    for spec in TESTS {
        let path = output_dir.join(format!("{}.log", spec.name));
        if let Ok(contents) = fs::read_to_string(&path) {
            let lines = contents.lines().collect::<Vec<_>>();
            let start = lines.len().saturating_sub(MAX_LOG_TAIL_LINES);
            println!(
                "--- {} 最近日志 ---\n{}",
                spec.name,
                lines[start..].join("\n")
            );
        }
    }
}
