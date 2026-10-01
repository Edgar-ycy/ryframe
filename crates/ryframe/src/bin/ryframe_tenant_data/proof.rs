use ryframe_application::ports::backup::{
    BackupManifest, BackupRepository, RestoreBusinessProof, RestoreRecord, RestoreStatus,
    validate_restore_record,
};
use ryframe_kernel::{AppError, AppResult};
use serde::Serialize;
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const MAX_EVIDENCE_BYTES: u64 = 16 * 1024 * 1024;
const VERIFIER_TIMEOUT: Duration = Duration::from_secs(300);
const COMPILED_SOURCE_SHA: Option<&str> = option_env!("RYFRAME_BUILD_COMMIT");

pub struct EvidencePaths<'a> {
    pub proof: &'a Path,
    pub tests: &'a Path,
    pub runtime: &'a Path,
    pub target: &'a Path,
    pub runner: &'a Path,
}

#[derive(Serialize)]
struct VerificationAuthority<'a> {
    format_version: u8,
    kind: &'static str,
    record: &'a RestoreRecord,
    backup_manifest: &'a BackupManifest,
    tools: VerificationTools,
}

#[derive(Serialize)]
struct VerificationTools {
    runner: GitSource,
    verifier: GitSource,
    python: ArtifactSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct GitSource {
    root: String,
    sha: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct ArtifactSource {
    path: String,
    bytes: u64,
    sha256: String,
}

struct CapturedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
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
    let backend = backend_root()?;
    let runner_root = verified_root(paths.runner, "恢复测试 runner")?;
    let verifier = clean_git_source(&backend, "恢复证明协调后端")?;
    if COMPILED_SOURCE_SHA != Some(verifier.sha.as_str()) {
        return Err(validation(
            "恢复证明维护程序必须由当前干净后端通过统一 data 入口构建",
        ));
    }
    let runner = clean_git_source(&runner_root, "恢复测试 runner")?;
    let python = verified_python(&backend)?;
    let before = read_regular_file(paths.proof, "恢复业务证明")?;
    let proof: RestoreBusinessProof = serde_json::from_slice(&before)
        .map_err(|error| validation(format!("恢复业务证明格式无效：{error}")))?;
    if proof.restore_id != id
        || proof.verifier_sha != verifier.sha
        || proof.runner_sha != runner.sha
    {
        return Err(validation(
            "恢复业务证明与命令行演练或独立采集的核验源码不同",
        ));
    }
    let authority = serde_json::to_vec(&VerificationAuthority {
        format_version: 1,
        kind: "restore-business-authority",
        record: &record,
        backup_manifest: &backup.manifest,
        tools: VerificationTools {
            runner: runner.clone(),
            verifier: verifier.clone(),
            python: python.clone(),
        },
    })
    .map_err(|_| validation("无法序列化恢复业务权威上下文"))?;
    let verified = run_verifier(&backend, &runner_root, &python, &paths, &authority)?;
    if verified != proof || read_regular_file(paths.proof, "恢复业务证明")? != before {
        return Err(validation(
            "恢复业务证明与独立核验结果不同或核验期间发生变化",
        ));
    }
    if clean_git_source(&backend, "恢复证明协调后端")? != verifier
        || clean_git_source(&runner_root, "恢复测试 runner")? != runner
        || artifact_source(Path::new(&python.path), "Python 解释器")? != python
    {
        return Err(validation("恢复证明核验期间工具或源码发生变化"));
    }
    Ok(proof)
}

fn run_verifier(
    backend: &Path,
    runner: &Path,
    python: &ArtifactSource,
    paths: &EvidencePaths<'_>,
    authority: &[u8],
) -> AppResult<RestoreBusinessProof> {
    let script = backend.join("tools/python/restore_business_proof.py");
    regular_path(&script, "恢复业务证明核验器")?;
    let scripts = script
        .parent()
        .ok_or_else(|| validation("无法确定恢复证明脚本目录"))?;
    let mut command = Command::new(&python.path);
    command
        .arg("-I")
        .arg("-c")
        .arg("import runpy,sys;root=sys.argv[1];script=sys.argv[2];sys.path.insert(0,root);sys.argv=[script,*sys.argv[3:]];runpy.run_path(script,run_name='__main__')")
        .arg(scripts)
        .arg(&script)
        .arg("--backend-dir")
        .arg(backend)
        .arg("--runner-root")
        .arg(runner)
        .arg("--proof")
        .arg(paths.proof)
        .arg("--tests-receipt")
        .arg(paths.tests)
        .arg("--runtime-receipt")
        .arg(paths.runtime)
        .arg("--target-plan")
        .arg(paths.target)
        .current_dir(backend)
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = capture_command(&mut command, authority, VERIFIER_TIMEOUT)?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(2000)
            .collect::<String>();
        return Err(validation(format!("恢复业务证明独立核验失败：{detail}")));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| validation(format!("恢复业务证明核验结果无效：{error}")))
}

fn backend_root() -> AppResult<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| validation("无法确定恢复证明协调后端根目录"))?;
    verified_root(root, "恢复证明协调后端")
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

fn verified_python(backend: &Path) -> AppResult<ArtifactSource> {
    let configured = std::env::var_os("RYFRAME_PYTHON").filter(|value| !value.is_empty());
    let path = if let Some(value) = configured {
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err(validation("RYFRAME_PYTHON 必须是绝对解释器路径"));
        }
        path
    } else {
        let mut command = Command::new("python");
        command
            .args([
                "-I",
                "-c",
                "import os,sys;print(os.path.realpath(sys.executable))",
            ])
            .env_remove("PYTHONHOME")
            .env_remove("PYTHONPATH");
        let output = capture_command(&mut command, &[], Duration::from_secs(30))?;
        if !output.status.success() {
            return Err(validation("无法解析默认 Python 解释器"));
        }
        let value = String::from_utf8(output.stdout)
            .map_err(|_| validation("Python 解释器路径不是 UTF-8"))?;
        PathBuf::from(value.trim())
    };
    let before = artifact_source(&path, "Python 解释器")?;
    let environment = backend.join("tools/python/check_python_environment.py");
    regular_path(&environment, "Python 环境核验器")?;
    let mut command = Command::new(&before.path);
    command
        .arg("-I")
        .arg(&environment)
        .current_dir(backend)
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH");
    let output = capture_command(&mut command, &[], Duration::from_secs(60))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(2000)
            .collect::<String>();
        return Err(validation(format!("固定 Python 环境核验失败：{detail}")));
    }
    if artifact_source(&path, "Python 解释器")? != before {
        return Err(validation("Python 解释器在环境核验期间发生变化"));
    }
    Ok(before)
}

fn artifact_source(path: &Path, label: &str) -> AppResult<ArtifactSource> {
    let artifact = super::proof_file::stable_artifact(path, label).map_err(validation)?;
    Ok(ArtifactSource {
        path: artifact.path,
        bytes: artifact.bytes,
        sha256: artifact.sha256,
    })
}

fn capture_command(
    command: &mut Command,
    stdin: &[u8],
    timeout: Duration,
) -> AppResult<CapturedOutput> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| validation("无法启动恢复核验子进程"))?;
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| validation("无法写入恢复核验子进程"))?
        .write_all(stdin);
    if write_result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(validation("无法写入恢复核验子进程"));
    }
    wait_with_output(&mut child, timeout)
}

fn wait_with_output(child: &mut Child, timeout: Duration) -> AppResult<CapturedOutput> {
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
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|_| validation("等待恢复核验子进程失败"))?
        {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(validation("恢复核验子进程超时"));
        }
        thread::sleep(Duration::from_millis(25));
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
        stderr: stderr.0,
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
