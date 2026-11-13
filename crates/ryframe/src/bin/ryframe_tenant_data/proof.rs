use ryframe_application::ports::backup::{
    BackupManifest, BackupRepository, RestoreBusinessProof, RestoreRecord, RestoreStatus,
    validate_restore_record,
};
use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::proof_binding::{ArtifactSource, GitSource, VerifierBinding};

const MAX_EVIDENCE_BYTES: u64 = 16 * 1024 * 1024;
const VERIFIER_TIMEOUT: Duration = Duration::from_secs(300);
fn compiled_source_sha() -> Option<&'static str> {
    (ryframe_kernel::BUILD_COMMIT != "development").then_some(ryframe_kernel::BUILD_COMMIT)
}

pub struct EvidencePaths<'a> {
    pub proof: &'a Path,
    pub tests: &'a Path,
    pub runtime: &'a Path,
    pub target: &'a Path,
    pub coordinator: &'a Path,
    pub coordinator_sha: &'a str,
    pub runner: &'a Path,
}

#[derive(Serialize)]
struct VerificationAuthority<'a> {
    format_version: u8,
    kind: &'static str,
    record: &'a RestoreRecord,
    backup_manifest: &'a BackupManifest,
    tools: VerificationTools<'a>,
}

#[derive(Serialize)]
struct VerificationTools<'a> {
    coordinator: &'a GitSource,
    runner: &'a GitSource,
    verifier: &'a GitSource,
    native_verifier: &'a ArtifactSource,
}

struct VerificationSources {
    coordinator: GitSource,
    runner: GitSource,
    verifier: GitSource,
    binding: VerifierBinding,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeResponse {
    ok: bool,
    result: NativeResult,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeResult {
    value: RestoreBusinessProof,
}

struct CapturedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
}

pub async fn verify(
    repository: &dyn BackupRepository,
    id: &str,
    paths: EvidencePaths<'_>,
) -> AppResult<RestoreBusinessProof> {
    let record = repository
        .restore(id)
        .await?
        .ok_or_else(|| validation("恢复演练不存在"))?;
    validate_restore_record(&record)?;
    if record.status != RestoreStatus::DataVerified {
        return Err(AppError::Conflict(
            "恢复数据验证通过后才能核验业务证明".into(),
        ));
    }
    let backup = repository
        .backup(&record.plan.backup_id)
        .await?
        .filter(|item| item.valid)
        .ok_or_else(|| validation("恢复演练引用的有效备份不存在"))?;
    let sources = VerificationSources::read(&paths)?;
    let native_verifier = sources.binding.verifier();
    let before = read_regular_file(paths.proof, "恢复业务证明")?;
    let proof: RestoreBusinessProof = serde_json::from_slice(&before)
        .map_err(|error| validation(format!("恢复业务证明格式无效：{error}")))?;
    if proof.restore_id != id
        || proof.verifier_sha != sources.verifier.sha
        || proof.runner_sha != sources.runner.sha
    {
        return Err(validation(
            "恢复业务证明与命令行演练或独立采集的核验源码不同",
        ));
    }
    let authority = VerificationAuthority {
        format_version: 2,
        kind: "restore-business-authority",
        record: &record,
        backup_manifest: &backup.manifest,
        tools: VerificationTools {
            coordinator: &sources.coordinator,
            runner: &sources.runner,
            verifier: &sources.verifier,
            native_verifier,
        },
    };
    let verified = run_verifier(
        Path::new(&sources.coordinator.root),
        Path::new(&sources.runner.root),
        native_verifier,
        &paths,
        &authority,
    )?;
    if verified != proof || read_regular_file(paths.proof, "恢复业务证明")? != before {
        return Err(validation(
            "恢复业务证明与独立核验结果不同或核验期间发生变化",
        ));
    }
    sources.check()?;
    Ok(proof)
}

impl VerificationSources {
    fn read(paths: &EvidencePaths<'_>) -> AppResult<Self> {
        let verifier_root = backend_root()?;
        let verifier = clean_git_source(&verifier_root, "恢复证明核验后端")?;
        if compiled_source_sha() != Some(verifier.sha.as_str()) {
            return Err(validation(
                "恢复证明维护程序必须由当前干净核验源码通过统一 data 入口构建",
            ));
        }
        let coordinator_root = verified_root(paths.coordinator, "恢复证明协调后端")?;
        let coordinator = clean_git_source(&coordinator_root, "恢复证明协调后端")?;
        if coordinator.sha != paths.coordinator_sha {
            return Err(validation("恢复证明协调工作树与 coordinator-sha 不一致"));
        }
        let runner_root = verified_root(paths.runner, "恢复测试 runner")?;
        let runner = clean_git_source(&runner_root, "恢复测试 runner")?;
        let binding = VerifierBinding::read(&verifier_root, &verifier.sha, &coordinator)?;
        Ok(Self {
            coordinator,
            runner,
            verifier,
            binding,
        })
    }

    fn check(&self) -> AppResult<()> {
        for (source, label) in [
            (&self.coordinator, "恢复证明协调后端"),
            (&self.verifier, "恢复证明核验后端"),
            (&self.runner, "恢复测试 runner"),
        ] {
            if clean_git_source(Path::new(&source.root), label)? != *source {
                return Err(validation("恢复证明核验期间源码发生变化"));
            }
        }
        self.binding.check()
    }
}

fn run_verifier(
    backend: &Path,
    runner: &Path,
    verifier: &ArtifactSource,
    paths: &EvidencePaths<'_>,
    authority: &VerificationAuthority<'_>,
) -> AppResult<RestoreBusinessProof> {
    let input = serde_json::to_vec(&json!({
        "operation":"restore-business-proof", "write":false,
        "arguments":{"backend":backend,"runner_root":runner,"proof":paths.proof,
            "tests_receipt":paths.tests,"runtime_receipt":paths.runtime,"target_plan":paths.target,
            "authority":authority},
    }))
    .map_err(|_| validation("无法序列化恢复业务权威上下文"))?;
    if input.len() as u64 > MAX_EVIDENCE_BYTES {
        return Err(validation("恢复业务权威上下文超过 16 MiB"));
    }
    let mut command = Command::new(&verifier.path);
    command
        .arg("__source-evidence")
        .current_dir(backend)
        .env_clear();
    for key in [
        "PATH",
        "SYSTEMROOT",
        "WINDIR",
        "PATHEXT",
        "COMSPEC",
        "USERPROFILE",
        "HOME",
        "HOMEDRIVE",
        "HOMEPATH",
        "TEMP",
        "TMP",
        "TMPDIR",
        "LANG",
        "LC_ALL",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = capture_command(&mut command, input, VERIFIER_TIMEOUT)?;
    if !output.status.success() {
        return Err(validation(format!(
            "恢复业务证明独立核验失败（退出状态 {}）",
            output.status
        )));
    }
    let response: NativeResponse = serde_json::from_slice(&output.stdout)
        .map_err(|_| validation("恢复业务证明核验结果格式无效"))?;
    if !response.ok {
        return Err(validation("恢复业务证明未获独立确认"));
    }
    Ok(response.result.value)
}

fn backend_root() -> AppResult<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| validation("无法确定恢复证明核验后端根目录"))?;
    verified_root(root, "恢复证明核验后端")
}

fn verified_root(path: &Path, label: &str) -> AppResult<PathBuf> {
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(validation(format!("{label}必须是规范绝对目录")));
    }
    reject_link_ancestors(path, label)?;
    let canonical =
        std::fs::canonicalize(path).map_err(|_| validation(format!("{label}目录不存在")))?;
    if !canonical.is_dir() {
        return Err(validation(format!("{label}必须是规范绝对目录")));
    }
    Ok(path.to_path_buf())
}

fn clean_git_source(root: &Path, label: &str) -> AppResult<GitSource> {
    let git = |arguments: &[&str]| -> AppResult<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .map_err(|_| validation(format!("无法核验{label} Git 来源")))?;
        if !output.status.success() {
            return Err(validation(format!("{label}不是可核验 Git 工作树")));
        }
        String::from_utf8(output.stdout)
            .map(|value| value.trim().to_owned())
            .map_err(|_| validation(format!("{label} Git 输出不是 UTF-8")))
    };
    let top_value = git(&["rev-parse", "--show-toplevel"])?;
    let top = verified_root(Path::new(&top_value), label)?;
    let sha = git(&["rev-parse", "HEAD"])?;
    if std::fs::canonicalize(&top).ok() != std::fs::canonicalize(root).ok()
        || !is_hex(&sha, 40)
        || !git(&["status", "--porcelain", "--untracked-files=all"])?.is_empty()
    {
        return Err(validation(format!(
            "{label}必须是证明绑定 SHA 的干净工作树"
        )));
    }
    Ok(GitSource {
        root: root.display().to_string(),
        sha,
    })
}

fn capture_command(
    command: &mut Command,
    input: Vec<u8>,
    timeout: Duration,
) -> AppResult<CapturedOutput> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|_| validation("无法启动恢复核验子进程"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| validation("无法写入恢复核验子进程"))?;
    let writer = thread::spawn(move || stdin.write_all(&input));
    let output = wait_with_output(&mut child, started, timeout)?;
    if writer
        .join()
        .map_err(|_| validation("写入恢复核验子进程失败"))?
        .is_err()
        && output.status.success()
    {
        return Err(validation("无法写入恢复核验子进程"));
    }
    Ok(output)
}

fn wait_with_output(
    child: &mut Child,
    started: Instant,
    timeout: Duration,
) -> AppResult<CapturedOutput> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| validation("恢复核验子进程缺少 stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| validation("恢复核验子进程缺少 stderr"))?;
    let stdout = thread::spawn(move || read_bounded_output(stdout));
    let stderr = thread::spawn(move || read_bounded_output(stderr));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(25)),
            observed => {
                let _ = child.kill();
                let _ = child.wait();
                let reason = if observed.is_err() {
                    "等待恢复核验子进程失败"
                } else {
                    "恢复核验子进程超时"
                };
                return Err(validation(reason));
            }
        }
    };
    let stdout = stdout
        .join()
        .map_err(|_| validation("读取恢复核验 stdout 失败"))??;
    let stderr = stderr
        .join()
        .map_err(|_| validation("读取恢复核验 stderr 失败"))??;
    if stdout.1 || stderr.1 {
        return Err(validation("恢复核验子进程输出超过 16 MiB"));
    }
    Ok(CapturedOutput {
        status,
        stdout: stdout.0,
    })
}

fn read_bounded_output(mut input: impl Read) -> AppResult<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut overflow = false;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|_| validation("读取恢复核验子进程输出失败"))?;
        if count == 0 {
            break;
        }
        let available = (MAX_EVIDENCE_BYTES as usize).saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(available)]);
        overflow |= count > available;
    }
    Ok((output, overflow))
}

fn read_regular_file(path: &Path, label: &str) -> AppResult<Vec<u8>> {
    let metadata = regular_path(path, label)?;
    if metadata.len() == 0 || metadata.len() > MAX_EVIDENCE_BYTES {
        return Err(validation(format!("{label}必须是 16 MiB 内的非空普通文件")));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let file = File::open(path).map_err(|_| validation(format!("{label}无法读取")))?;
    let opened = same_file::Handle::from_file(
        file.try_clone()
            .map_err(|_| validation(format!("{label}无法核验")))?,
    )
    .map_err(|_| validation(format!("{label}无法核验")))?;
    if opened
        != same_file::Handle::from_path(path).map_err(|_| validation(format!("{label}无法核验")))?
    {
        return Err(validation(format!("{label}在打开前被替换")));
    }
    file.take(MAX_EVIDENCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| validation(format!("{label}无法读取")))?;
    let after = regular_path(path, label)?;
    if bytes.len() as u64 != metadata.len()
        || after.len() != metadata.len()
        || opened
            != same_file::Handle::from_path(path)
                .map_err(|_| validation(format!("{label}无法核验")))?
        || after.modified().ok() != metadata.modified().ok()
    {
        return Err(validation(format!("{label}读取期间发生变化")));
    }
    Ok(bytes)
}

fn regular_path(path: &Path, label: &str) -> AppResult<std::fs::Metadata> {
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(validation(format!("{label}必须是规范绝对文件路径")));
    }
    reject_link_ancestors(path, label)?;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| validation(format!("{label}不存在")))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(validation(format!("{label}必须是普通文件")));
    }
    Ok(metadata)
}

fn reject_link_ancestors(path: &Path, label: &str) -> AppResult<()> {
    for ancestor in path.ancestors() {
        let Ok(metadata) = std::fs::symlink_metadata(ancestor) else {
            continue;
        };
        if metadata.file_type().is_symlink() || is_reparse(&metadata) {
            return Err(validation(format!("{label}路径不能经过链接或重解析点")));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(_: &std::fs::Metadata) -> bool {
    false
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validation(message: impl Into<String>) -> AppError {
    AppError::Validation(message.into())
}
