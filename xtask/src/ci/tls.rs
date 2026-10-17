use std::{
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use crate::Result;

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

const LOOPBACK_HOST: &str = "127.0.0.1";
const LOOPBACK_NAMES: &[&str] = &["127.0.0.1", "::1", "localhost"];
const DEFAULT_REDIS_PORT: u16 = 6379;
const DEFAULT_REDIS_DATABASE: &str = "15";
const DEFAULT_TIMEOUT_SECONDS: u64 = 900;
const MAX_LOG_TAIL_LINES: usize = 80;
const ARTIFACT_ENV: &str = "RYFRAME_TLS_ARTIFACT_DIR";
const RUN_ID_ENV: &str = "RYFRAME_INTEGRATION_RUN_ID";
const DEFAULT_ARTIFACT_ROOT: &str = ".local-tests/integration/tls";

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

struct GateOptions {
    backend_root: PathBuf,
    target_dir: PathBuf,
    jobs: usize,
    timeout: Duration,
    artifact_root: PathBuf,
}

pub(crate) struct TestEnvironment {
    pub(crate) values: Vec<(String, String)>,
    pub(crate) removed: Vec<&'static str>,
}

pub(crate) fn run_from_environment(
    backend_root: &Path,
    target_dir: &Path,
    jobs: usize,
) -> Result<PathBuf> {
    let artifact_root = env::var_os(ARTIFACT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ARTIFACT_ROOT));
    run_gate(GateOptions {
        backend_root: backend_root.to_path_buf(),
        target_dir: target_dir.to_path_buf(),
        jobs: jobs.max(1),
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
        artifact_root: if artifact_root.is_absolute() {
            artifact_root
        } else {
            backend_root.join(artifact_root)
        },
    })
}

fn run_gate(options: GateOptions) -> Result<PathBuf> {
    let run_id = env::var(RUN_ID_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("local-{}", std::process::id()));
    let output_dir = reserve_output_dir(&options.artifact_root, &run_id)?;
    println!("tls_integration_artifacts={}", output_dir.display());
    let material_dir = output_dir.join("fixture-material");
    let mut failures = Vec::new();
    let mut fixtures = None;
    let mut ca_cert = None;

    let started = (|| -> Result<()> {
        let endpoint = redis_endpoint_from_environment()?;
        let upstream = check_redis(&endpoint)?;
        fs::create_dir(&material_dir).map_err(|error| {
            format!(
                "无法创建 TLS fixture 证书目录 {}：{error}",
                material_dir.display()
            )
        })?;
        let material =
            certificate::generate_certificates(&material_dir, &output_dir.join("openssl.log"))?;
        ca_cert = Some(material.ca_cert.clone());
        let servers = fixture::FixtureServers::start(
            &material.server_cert,
            &material.server_key,
            upstream,
            &output_dir.join("fixtures.log"),
        )?;
        fixtures = Some(servers);
        Ok(())
    })();
    if let Err(error) = started {
        failures.push(error.to_string());
    } else {
        let servers = fixtures
            .as_ref()
            .expect("成功启动 TLS fixture 后必须保存服务句柄");
        let environment = test_environment(
            ca_cert
                .as_ref()
                .expect("成功生成 TLS fixture 证书后必须保存 CA"),
            servers.redis_port(),
            servers.https_port(),
            env::var("RYFRAME_REDIS_DATABASE").ok(),
        );
        failures.extend(collect_test_failures(|spec| {
            run_test(
                spec,
                &options,
                &environment,
                &output_dir.join(format!("{}.log", spec.name)),
            )
        }));
    }

    if let Some(mut servers) = fixtures
        && let Err(error) = servers.stop()
    {
        failures.push(error.to_string());
    }
    if let Err(error) = remove_material(&output_dir, &material_dir) {
        failures.push(error.to_string());
    }
    write_summary(&output_dir, &failures)?;
    if failures.is_empty() {
        Ok(output_dir)
    } else {
        print_failure_tails(&output_dir);
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

fn run_test(
    spec: &TestSpec,
    options: &GateOptions,
    environment: &TestEnvironment,
    log_path: &Path,
) -> Option<String> {
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
    command.envs(environment.values.iter().map(|(key, value)| (key, value)));
    for name in &environment.removed {
        command.env_remove(name);
    }
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

pub(crate) fn test_environment(
    ca_cert: &Path,
    redis_port: u16,
    https_port: u16,
    redis_database: Option<String>,
) -> TestEnvironment {
    let ca = ca_cert.to_string_lossy().into_owned();
    let database = redis_database
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REDIS_DATABASE.to_owned());
    TestEnvironment {
        values: vec![
            ("NO_PROXY".to_owned(), "127.0.0.1,localhost".to_owned()),
            ("no_proxy".to_owned(), "127.0.0.1,localhost".to_owned()),
            ("SSL_CERT_FILE".to_owned(), ca.clone()),
            ("RYFRAME_REDIS_HOST".to_owned(), LOOPBACK_HOST.to_owned()),
            ("RYFRAME_REDIS_PORT".to_owned(), redis_port.to_string()),
            ("RYFRAME_REDIS_DATABASE".to_owned(), database),
            ("RYFRAME_REDIS_TLS".to_owned(), "1".to_owned()),
            ("RYFRAME_REDIS_TLS_CA".to_owned(), ca.clone()),
            ("RYFRAME_REDIS_TLS_INTEGRATION".to_owned(), "1".to_owned()),
            ("RYFRAME_HTTPS_CA_PEM".to_owned(), ca),
            ("RYFRAME_S3_HTTPS_INTEGRATION".to_owned(), "1".to_owned()),
            (
                "RYFRAME_S3_HTTPS_ENDPOINT".to_owned(),
                format!("https://{LOOPBACK_HOST}:{https_port}"),
            ),
            ("RYFRAME_S3_HTTPS_REGION".to_owned(), "us-east-1".to_owned()),
            ("RYFRAME_OTLP_HTTPS_INTEGRATION".to_owned(), "1".to_owned()),
            (
                "RYFRAME_OTLP_HTTPS_ENDPOINT".to_owned(),
                format!("https://{LOOPBACK_HOST}:{https_port}/v1/traces"),
            ),
        ],
        removed: vec![
            "RYFRAME_REDIS_TLS_CLIENT_CERT",
            "RYFRAME_REDIS_TLS_CLIENT_KEY",
            "SSL_CERT_DIR",
        ],
    }
}

fn redis_endpoint_from_environment() -> Result<RedisEndpoint> {
    let host = env::var("RYFRAME_REDIS_HOST")
        .ok()
        .unwrap_or_else(|| LOOPBACK_HOST.to_owned());
    let port = env::var("RYFRAME_REDIS_PORT")
        .ok()
        .unwrap_or_else(|| DEFAULT_REDIS_PORT.to_string());
    redis_endpoint(&host, &port)
}

pub(crate) fn redis_endpoint(host: &str, raw_port: &str) -> Result<RedisEndpoint> {
    let host = host.trim();
    if !LOOPBACK_NAMES.contains(&host) {
        return Err("TLS 门禁只允许连接回环 Redis，拒绝接触共享远程服务".into());
    }
    let port = raw_port
        .trim()
        .parse::<u16>()
        .map_err(|error| format!("RYFRAME_REDIS_PORT 不是有效端口 {raw_port:?}：{error}"))?;
    if port == 0 {
        return Err("RYFRAME_REDIS_PORT 必须在 1 到 65535 之间，实际为 0".into());
    }
    Ok(RedisEndpoint {
        host: host.to_owned(),
        port,
    })
}

pub(crate) fn check_redis(endpoint: &RedisEndpoint) -> Result<SocketAddr> {
    let mut errors = Vec::new();
    for address in fixture::resolved_endpoints(endpoint)? {
        match probe_redis(address) {
            Ok(()) => return Ok(address),
            Err(error) => errors.push(format!("{address}: {error}")),
        }
    }
    Err(format!(
        "回环 Redis {}:{} 不可用：{}",
        endpoint.host,
        endpoint.port,
        errors.join("; ")
    )
    .into())
}

fn probe_redis(address: SocketAddr) -> Result<()> {
    let mut connection = TcpStream::connect_timeout(&address, Duration::from_secs(3))
        .map_err(|error| error.to_string())?;
    connection
        .set_read_timeout(Some(Duration::from_secs(3)))
        .and_then(|_| connection.set_write_timeout(Some(Duration::from_secs(3))))
        .map_err(|error| format!("无法设置探测超时：{error}"))?;
    connection
        .write_all(b"*1\r\n$4\r\nPING\r\n")
        .and_then(|_| connection.flush())
        .map_err(|error| format!("无法发送 PING：{error}"))?;
    let mut response = [0_u8; 256];
    let read = connection
        .read(&mut response)
        .map_err(|error| format!("无法读取 PING 响应：{error}"))?;
    let response = &response[..read];
    if response.starts_with(b"+PONG") || response.starts_with(b"-NOAUTH") {
        Ok(())
    } else {
        Err(format!("未返回预期 Redis PING 响应：{response:?}").into())
    }
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
