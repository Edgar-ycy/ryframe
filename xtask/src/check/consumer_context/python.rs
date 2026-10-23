//! 由工具文件选择 Python 入口，观察运行身份时不替换虚拟环境入口。

use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::{
    Result,
    process::{ChildGroup, PreservedFailure, stop_child},
};

const PROBE: &str = "import json,sys;print(json.dumps({'executable':sys.executable,'version':list(sys.version_info[:3])}))";
const PYTHON_ERROR: &str = "Python 入口核验失败：verification.python_program 请使用无父目录跳转（ParentDir）、无控制字符的绝对路径或以后端根目录解析的相对路径；缺省时核对操作系统 PATH 返回的无控制字符绝对入口与 Python 3.11+，保留现场后重试。";
const OUTPUT_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    executable: PathBuf,
    version: [u32; 3],
}

pub(crate) fn selected_program(root: &Path) -> Result<PathBuf> {
    super::files::raw_path(root)?;
    if !root.is_absolute() {
        return Err(PYTHON_ERROR.into());
    }
    let settings = ryframe_config::ToolSettings::load(root)?;
    let configured = settings.verification.python_program;
    let requested = requested_program(root, configured.as_deref())?;
    let output = output(
        root,
        command(root, &requested, &["-I", "-c", PROBE]),
        None,
        Duration::from_secs(30),
    )?;
    if !output.status.success() {
        return Err(
            PreservedFailure::new(PYTHON_ERROR.into(), output.status.code().or(Some(1))).into(),
        );
    }
    let observed: Observation = serde_json::from_slice(&output.stdout).map_err(|_| PYTHON_ERROR)?;
    if !observed.executable.is_absolute()
        || !valid_program_path(&observed.executable)
        || observed.version < [3, 11, 0]
        || !observed.executable.is_file()
    {
        return Err(PYTHON_ERROR.into());
    }
    // 不用 canonicalize 的 base 可执行文件替换已选择的 venv 入口。
    Ok(if configured.is_some() {
        requested
    } else {
        observed.executable
    })
}

fn requested_program(root: &Path, configured: Option<&Path>) -> Result<PathBuf> {
    if configured.is_some_and(|path| !valid_program_path(path)) {
        return Err(PYTHON_ERROR.into());
    }
    match configured {
        Some(path) if path.is_absolute() => Ok(path.to_owned()),
        Some(path) => std::path::absolute(root.join(path)).map_err(|_| PYTHON_ERROR.into()),
        None => Ok(PathBuf::from("python")),
    }
}

fn valid_program_path(path: &Path) -> bool {
    path.to_str().is_some_and(|text| {
        !text.is_empty()
            && !text.chars().any(char::is_control)
            && !text.split(['/', '\\']).any(|component| component == "..")
    }) && !path.components().any(|component| {
        matches!(component, std::path::Component::ParentDir)
            || component.as_os_str() == std::ffi::OsStr::new("..")
    })
}

pub(crate) fn command(root: &Path, program: &Path, arguments: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

pub(crate) fn output(
    root: &Path,
    mut command: Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Output> {
    if let Some(input) = input {
        let scratch = root.join(".local-tests");
        super::files::directory(&scratch)?;
        let mut file = tempfile::tempfile_in(scratch)?;
        file.write_all(input)?;
        file.seek(SeekFrom::Start(0))?;
        command.stdin(Stdio::from(file));
    }
    let mut child = ChildGroup::new()?.spawn(command)?;
    let stdout = reader(child.take_stdout_reader()?);
    let stderr = reader(child.take_stderr_reader()?);
    let deadline = Instant::now() + timeout;
    let status: Result<std::process::ExitStatus> = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                break Err(
                    "Python/原生工具协议超时：已停止进程树，请核对入口及固定探针后重试。".into(),
                );
            }
            Err(_) => break Err(PYTHON_ERROR.into()),
        }
    };
    let cleanup = stop_child(&mut child);
    drop(child);
    let stdout = join_reader(stdout);
    let stderr = join_reader(stderr);
    cleanup?;
    Ok(Output {
        status: status?,
        stdout: stdout?,
        stderr: stderr?,
    })
}

fn reader(stream: Option<Box<dyn Read + Send>>) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(stream) = stream {
            stream.take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes)?;
        }
        Ok(bytes)
    })
}

fn join_reader(reader: thread::JoinHandle<std::io::Result<Vec<u8>>>) -> Result<Vec<u8>> {
    let bytes = reader
        .join()
        .map_err(|_| PYTHON_ERROR)?
        .map_err(|_| PYTHON_ERROR)?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        return Err("工具协议输出超限：请检查解释器与原生工具入口。".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests/consumer-python-tests");
        std::fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }

    #[test]
    fn configured_relative_venv_entry_is_resolved_without_canonical_replacement() {
        let root = fixture();
        let relative = Path::new("venv/Scripts/python.exe");
        let selected = requested_program(root.path(), Some(relative)).unwrap();
        assert_eq!(selected, root.path().join(relative));
        assert!(selected.is_absolute());
        assert_eq!(
            requested_program(root.path(), None).unwrap(),
            Path::new("python")
        );
        assert!(requested_program(root.path(), Some(Path::new(""))).is_err());
        let command = command(root.path(), &selected, &["--version"]);
        assert_eq!(command.get_program(), selected.as_os_str());
    }

    #[test]
    fn explicit_parent_traversal_and_control_characters_fail_without_disclosure() {
        let root = fixture();
        for value in [
            "../SECRET_SENTINEL/python",
            "venv/../SECRET_SENTINEL",
            r"venv\..\SECRET_SENTINEL",
            "venv/SECRET_SENTINEL\npython",
            "venv/SECRET_SENTINEL\u{85}python",
        ] {
            let error = requested_program(root.path(), Some(Path::new(value)))
                .unwrap_err()
                .to_string();
            assert!(error.contains("verification.python_program"));
            assert!(error.contains("ParentDir"));
            assert!(!error.contains("SECRET_SENTINEL"));
        }
        assert!(!valid_program_path(Path::new("/tmp/python\r")));
    }

    #[test]
    fn local_tool_configuration_overrides_public_python_selection() {
        let root = fixture();
        let observed = selected_program(&crate::workspace::root_dir()).unwrap();
        std::fs::create_dir(root.path().join("config")).unwrap();
        let public = root.path().join("config/tools.toml");
        std::fs::write(&public, "[verification]\npython_program='missing-python'\n").unwrap();
        assert!(selected_program(root.path()).is_err());
        let document = serde_json::json!({"verification": {"python_program": observed}});
        let configuration = toml::to_string(&document).unwrap();
        std::fs::create_dir(root.path().join(".local-tests")).unwrap();
        let local = root.path().join(".local-tests/tools.toml");
        std::fs::write(&local, &configuration).unwrap();
        assert_eq!(selected_program(root.path()).unwrap(), observed);
        std::fs::remove_file(local).unwrap();
        assert!(selected_program(root.path()).is_err());
        std::fs::write(public, configuration).unwrap();
        assert_eq!(selected_program(root.path()).unwrap(), observed);
    }

    #[test]
    fn python_selection_and_test_callers_have_no_legacy_environment_protocol() {
        let root = crate::workspace::root_dir();
        let legacy = ["RYFRAME", "PYTHON"].join("_");
        for path in [
            "xtask/src/check/consumer_context/python.rs",
            "xtask/src/cli/help.rs",
            "xtask/tests/cli_process.rs",
            "xtask/tests/cli_process/security_reports.rs",
            "xtask/tests/internal/target_reset_native.rs",
            "xtask/tests/internal/target_proof_native.rs",
            "xtask/tests/internal/target_guard_native.rs",
            "xtask/tests/internal/clone_bridge.rs",
            "tools/python/test_restore_reference_dataset_cli.py",
        ] {
            let source = std::fs::read_to_string(root.join(path)).unwrap();
            assert!(!source.contains(&legacy), "{path} 仍含旧解释器环境入口");
        }
    }
}
