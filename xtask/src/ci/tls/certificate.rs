use std::{
    env,
    ffi::OsString,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use super::process::{ProcessOutcome, run_redirected};
use crate::Result;

const OPENSSL_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) struct CertificateMaterial {
    pub(crate) ca_cert: PathBuf,
    pub(crate) server_cert: PathBuf,
    pub(crate) server_key: PathBuf,
}

pub(crate) fn generate_certificates(
    directory: &Path,
    log_path: &Path,
) -> Result<CertificateMaterial> {
    let program = env::var_os("RYFRAME_OPENSSL")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("openssl"));
    generate_certificates_with_program(program, directory, log_path)
}

pub(crate) fn generate_certificates_with_program(
    program: OsString,
    directory: &Path,
    log_path: &Path,
) -> Result<CertificateMaterial> {
    let ca_config = directory.join("ca.cnf");
    let server_config = directory.join("server.cnf");
    write_config(&ca_config, CA_CONFIG)?;
    write_config(&server_config, SERVER_CONFIG)?;

    let ca_key = directory.join("ca-key.pem");
    let ca_cert = directory.join("ca.pem");
    let server_key = directory.join("server-key.pem");
    let server_request = directory.join("server.csr");
    let server_cert = directory.join("server.pem");
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .map_err(|error| format!("无法创建 OpenSSL 日志 {}：{error}", log_path.display()))?;
    for command in [
        openssl_command(
            &program,
            [
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-sha256",
                "-nodes",
                "-days",
                "2",
                "-config",
                &path_argument(&ca_config),
                "-keyout",
                &path_argument(&ca_key),
                "-out",
                &path_argument(&ca_cert),
            ],
        ),
        openssl_command(
            &program,
            [
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-sha256",
                "-nodes",
                "-config",
                &path_argument(&server_config),
                "-keyout",
                &path_argument(&server_key),
                "-out",
                &path_argument(&server_request),
            ],
        ),
        openssl_command(
            &program,
            [
                "x509",
                "-req",
                "-sha256",
                "-days",
                "2",
                "-in",
                &path_argument(&server_request),
                "-CA",
                &path_argument(&ca_cert),
                "-CAkey",
                &path_argument(&ca_key),
                "-CAcreateserial",
                "-extfile",
                &path_argument(&server_config),
                "-extensions",
                "v3_req",
                "-out",
                &path_argument(&server_cert),
            ],
        ),
    ] {
        match run_redirected(command, &mut log, OPENSSL_TIMEOUT) {
            Ok(ProcessOutcome::Exited(status)) if status.success() => {}
            Ok(ProcessOutcome::Exited(status)) => {
                return Err(format!(
                    "生成 TLS fixture 证书失败，退出码 {}；详见 {}",
                    status
                        .code()
                        .map_or_else(|| status.to_string(), |code| code.to_string(),),
                    log_path.display()
                )
                .into());
            }
            Ok(ProcessOutcome::TimedOut) => {
                return Err(format!(
                    "OpenSSL 生成 TLS fixture 证书超过 {} 秒；详见 {}",
                    OPENSSL_TIMEOUT.as_secs(),
                    log_path.display()
                )
                .into());
            }
            Err(error) => {
                return Err(format!(
                    "无法执行 OpenSSL（{}）。Windows 可使用 Git for Windows 附带的 \
                     openssl，或通过 RYFRAME_OPENSSL 指定可执行文件；原因：{error}",
                    program.to_string_lossy()
                )
                .into());
            }
        }
    }
    Ok(CertificateMaterial {
        ca_cert,
        server_cert,
        server_key,
    })
}

fn write_config(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content.as_bytes())
        .map_err(|error| format!("无法写入 OpenSSL 配置 {}：{error}", path.display()))?;
    Ok(())
}

fn path_argument(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn openssl_command<const N: usize>(program: &OsString, arguments: [&str; N]) -> Command {
    let mut command = Command::new(program);
    command.args(arguments);
    command
}

const CA_CONFIG: &str = "[req]
prompt = no
distinguished_name = dn
x509_extensions = v3_ca
[dn]
CN = RyFrame Integration CA
[v3_ca]
basicConstraints = critical, CA:TRUE, pathlen:0
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
";

const SERVER_CONFIG: &str = "[req]
prompt = no
distinguished_name = dn
req_extensions = v3_req
[dn]
CN = localhost
[v3_req]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = @alt_names
[alt_names]
DNS.1 = localhost
IP.1 = 127.0.0.1
";
