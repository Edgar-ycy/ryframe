use super::process_liveness::process_is_running;

use std::{
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::process::{
    ChildGroup, command_output, command_output_with_env, configure_pnpm_environment,
    failure_exit_code, python_arguments, python_environment, resolved_executable, run, stop_child,
    with_process_log,
};

#[test]
fn process_liveness_distinguishes_the_current_and_invalid_pid() {
    assert!(process_is_running(std::process::id()));
    assert!(!process_is_running(u32::MAX));
}

#[test]
fn python_runner_honors_the_explicit_isolated_interpreter() {
    let root = tool_program_directory();
    assert_eq!(
        resolved_executable(root.path(), "python").unwrap(),
        "python"
    );
    assert_eq!(resolved_executable(root.path(), "node").unwrap(), "node");
    let configured = root.path().join("venv/Scripts/python.exe");
    let node = root.path().join("node/bin/node.exe");
    write_tool_programs(
        root.path(),
        "config/tools.toml",
        &[
            ("python_program", configured.to_str().unwrap()),
            ("node_program", node.to_str().unwrap()),
        ],
    );
    assert_eq!(
        resolved_executable(root.path(), "python").unwrap(),
        configured.into_os_string()
    );
    assert_eq!(
        resolved_executable(root.path(), "node").unwrap(),
        node.into_os_string()
    );
    assert_eq!(resolved_executable(root.path(), "cargo").unwrap(), "cargo");
}

#[test]
fn tool_program_files_apply_local_precedence_per_field() {
    let root = tool_program_directory();
    write_tool_programs(
        root.path(),
        "config/tools.toml",
        &[
            ("python_program", "public/python"),
            ("node_program", "public/node"),
        ],
    );
    for (program, relative) in [("python", "public/python"), ("node", "public/node")] {
        assert_eq!(
            resolved_executable(root.path(), program).unwrap(),
            std::path::absolute(root.path().join(relative))
                .unwrap()
                .into_os_string()
        );
    }
    write_tool_programs(
        root.path(),
        ".local-tests/tools.toml",
        &[("node_program", "本机/node")],
    );
    assert_eq!(
        resolved_executable(root.path(), "node").unwrap(),
        std::path::absolute(root.path().join("本机/node"))
            .unwrap()
            .into_os_string()
    );
    assert_eq!(
        resolved_executable(root.path(), "python").unwrap(),
        std::path::absolute(root.path().join("public/python"))
            .unwrap()
            .into_os_string()
    );
    write_tool_programs(
        root.path(),
        ".local-tests/tools.toml",
        &[
            ("python_program", "本机/python"),
            ("node_program", "本机/node"),
        ],
    );
    assert_eq!(
        resolved_executable(root.path(), "python").unwrap(),
        std::path::absolute(root.path().join("本机/python"))
            .unwrap()
            .into_os_string()
    );
}

#[test]
fn tool_program_relative_paths_keep_the_selected_venv_entry() {
    let root = tool_program_directory();
    let relative = ".local-tests/venv/Scripts/python.exe";
    let expected = std::path::absolute(root.path().join(relative)).unwrap();
    assert!(!expected.exists());
    write_tool_programs(
        root.path(),
        "config/tools.toml",
        &[("python_program", relative)],
    );
    assert_eq!(
        resolved_executable(root.path(), "python").unwrap(),
        expected.clone().into_os_string()
    );
    write_tool_programs(
        root.path(),
        "config/tools.toml",
        &[("python_program", expected.to_str().unwrap())],
    );
    assert_eq!(
        resolved_executable(root.path(), "python").unwrap(),
        expected.into_os_string()
    );
}

#[test]
fn other_process_programs_do_not_read_invalid_tool_files() {
    let root = tool_program_directory();
    fs::write(root.path().join("config/tools.toml"), "SECRET_SENTINEL=").unwrap();
    for program in [
        "cargo",
        "rustc",
        "python.exe",
        "node.exe",
        "absolute/python",
    ] {
        assert_eq!(resolved_executable(root.path(), program).unwrap(), program);
    }
    let explicit = root.path().join("bin/python.exe");
    assert_eq!(
        resolved_executable(root.path(), explicit.to_str().unwrap()).unwrap(),
        explicit.into_os_string()
    );
    assert!(resolved_executable(root.path(), "python").is_err());
    assert!(resolved_executable(root.path(), "node").is_err());
}

#[test]
fn tool_program_invalid_paths_and_files_never_disclose_values() {
    let root = tool_program_directory();
    for (program, field) in [("python", "python_program"), ("node", "node_program")] {
        for value in [
            "",
            "SECRET_SENTINEL\npython",
            "SECRET_SENTINEL\rpython",
            "SECRET_SENTINEL\tpython",
            "SECRET_SENTINEL\0python",
            "SECRET_SENTINEL\u{85}python",
            "SECRET_SENTINEL/../python",
            "SECRET_SENTINEL\\..\\python",
            "../SECRET_SENTINEL",
            "..\\SECRET_SENTINEL",
        ] {
            write_tool_programs(root.path(), "config/tools.toml", &[(field, value)]);
            let error = resolved_executable(root.path(), program)
                .unwrap_err()
                .to_string();
            assert!(error.contains(&format!("verification.{field}")));
            assert!(!error.contains("SECRET_SENTINEL"));
        }
        for (document, cause, remedy) in [
            (
                format!("[verification]\n{field}={{ password='SECRET_SENTINEL' }}"),
                "工具配置字段或类型错误",
                "字段名与类型",
            ),
            (
                "[verification]\nSECRET_SENTINEL='unknown'".into(),
                "工具配置字段或类型错误",
                "字段名与类型",
            ),
            (
                format!("[verification]\n{field}='SECRET_SENTINEL"),
                "TOML 格式错误，位置",
                "检查表名、引号和字段类型",
            ),
        ] {
            fs::write(root.path().join("config/tools.toml"), document).unwrap();
            assert_tool_program_load_error(
                root.path(),
                program,
                field,
                "config/tools.toml",
                cause,
                remedy,
            );
        }
    }
}

#[test]
fn invalid_local_tool_program_never_falls_back_to_public_selection() {
    let root = tool_program_directory();
    write_tool_programs(
        root.path(),
        "config/tools.toml",
        &[
            ("python_program", "public/python"),
            ("node_program", "public/node"),
        ],
    );
    for (program, field) in [("python", "python_program"), ("node", "node_program")] {
        write_tool_programs(root.path(), ".local-tests/tools.toml", &[(field, "")]);
        let error = resolved_executable(root.path(), program)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!("verification.{field}")));
        assert!(!error.contains("SECRET_SENTINEL"));
        for (document, cause, remedy) in [
            (
                format!("[verification]\n{field}={{ password='SECRET_SENTINEL' }}"),
                "工具配置字段或类型错误",
                "字段名与类型",
            ),
            (
                format!("[verification]\n{field}='SECRET_SENTINEL"),
                "TOML 格式错误，位置",
                "检查表名、引号和字段类型",
            ),
        ] {
            fs::write(root.path().join(".local-tests/tools.toml"), document).unwrap();
            assert_tool_program_load_error(
                root.path(),
                program,
                field,
                ".local-tests/tools.toml",
                cause,
                remedy,
            );
        }
    }
}

#[test]
fn tool_program_read_errors_keep_file_kind_and_repair_without_fallback() {
    for (program, field) in [("python", "python_program"), ("node", "node_program")] {
        for relative in ["config/tools.toml", ".local-tests/tools.toml"] {
            let root = tool_program_directory();
            write_tool_programs(
                root.path(),
                "config/tools.toml",
                &[(field, "public/program")],
            );
            let path = root.path().join(relative);
            if path.is_file() {
                fs::remove_file(&path).unwrap();
            }
            fs::create_dir(&path).unwrap();
            let kind = fs::read_to_string(&path).unwrap_err().kind().to_string();
            let cause = format!("无法读取工具配置 {}：{kind}", path.display());
            assert_tool_program_load_error(
                root.path(),
                program,
                field,
                relative,
                &cause,
                "检查文件路径及读取权限",
            );
            fs::remove_dir(&path).unwrap();
            fs::write(&path, b"SECRET_SENTINEL\xff").unwrap();
            let cause = format!(
                "无法读取工具配置 {}：{}",
                path.display(),
                std::io::ErrorKind::InvalidData,
            );
            assert_tool_program_load_error(
                root.path(),
                program,
                field,
                relative,
                &cause,
                "检查文件路径及读取权限",
            );
        }
    }
}

fn assert_tool_program_load_error(
    root: &Path,
    program: &str,
    field: &str,
    relative: &str,
    cause: &str,
    remedy: &str,
) {
    let safe_error = ryframe_config::ToolSettings::load(root)
        .unwrap_err()
        .to_string();
    let error = resolved_executable(root, program).unwrap_err().to_string();
    assert!(error.starts_with(&safe_error));
    for fragment in [
        relative,
        cause,
        remedy,
        &format!("verification.{field}"),
        "显式入口须非空、无控制字符和父目录跳转",
        "相对路径以后端根目录解析",
    ] {
        assert!(error.contains(fragment), "缺少安全配置原因或修复建议");
    }
    assert!(!error.contains("SECRET_SENTINEL"));
}

const FRONTEND_CWD_SUCCESS: &str = "frontend-staging-cwd-confirmed";
const BACKEND_DRIVER_SUCCESS: &str = "backend-driver-node-selection-confirmed";

#[test]
fn node_command_uses_driver_backend_settings_with_frontend_working_directory() {
    let root = tool_program_directory();
    fs::write(root.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir(root.path().join("xtask")).unwrap();
    let executable = std::env::current_exe().unwrap();
    write_tool_programs(
        root.path(),
        "config/tools.toml",
        &[("node_program", executable.to_str().unwrap())],
    );
    let frontend = root.path().join("frontend-staging");
    fs::create_dir_all(frontend.join("config")).unwrap();
    fs::write(
        frontend.join("config/tools.toml"),
        "INVALID_FRONTEND_CONFIG=",
    )
    .unwrap();
    let mut command = Command::new(executable);
    command
        .args([
            "--exact",
            "process_tests::backend_node_driver_probe",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .current_dir(root.path())
        .env_remove("RYFRAME_WORKSPACE_ROOT")
        .env_remove("RYFRAME_DEVEX_TARGET_ROOT")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ChildGroup::new().unwrap().spawn(command).unwrap();
    let readers = [
        child.take_stdout_reader().unwrap().unwrap(),
        child.take_stderr_reader().unwrap().unwrap(),
    ]
    .map(|mut stream| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).map(|_| bytes)
        })
    });
    let deadline = Instant::now() + Duration::from_secs(45);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => break Err("父测试等待 driver 超过 45 秒".to_owned()),
            Err(error) => break Err(format!("无法等待 driver：{error}")),
        }
    };
    let cleanup = stop_child(&mut child);
    drop(child);
    let [stdout, stderr] = readers.map(|reader| reader.join().unwrap().unwrap());
    cleanup.unwrap();
    let stdout = String::from_utf8(stdout).unwrap();
    let stderr = String::from_utf8_lossy(&stderr);
    let status = status.unwrap_or_else(|error| panic!("{error}\n{stdout}\n{stderr}"));
    assert!(status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.lines().any(|line| line == FRONTEND_CWD_SUCCESS),
        "{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line == BACKEND_DRIVER_SUCCESS),
        "{stdout}"
    );
}

#[test]
#[ignore = "仅由后端工具选择回归的父进程显式执行"]
fn backend_node_driver_probe() {
    let backend = std::env::current_dir().unwrap();
    let frontend = backend.join("frontend-staging");
    assert!(backend.join("Cargo.toml").is_file());
    assert!(backend.join("xtask").is_dir());
    let output = command_output(
        &frontend,
        "node",
        &[
            "--exact",
            "process_tests::frontend_node_inner_probe",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ],
    )
    .unwrap();
    assert!(
        output.lines().any(|line| line == FRONTEND_CWD_SUCCESS),
        "{output}"
    );
    print!("{output}");
    println!("\n{BACKEND_DRIVER_SUCCESS}");
}

#[test]
#[ignore = "仅由后端工具选择回归的 driver 显式执行"]
fn frontend_node_inner_probe() {
    let frontend = std::env::current_dir().unwrap();
    assert_eq!(frontend.file_name().unwrap(), "frontend-staging");
    assert_eq!(
        fs::read_to_string(frontend.join("config/tools.toml")).unwrap(),
        "INVALID_FRONTEND_CONFIG="
    );
    assert!(frontend.parent().unwrap().join("Cargo.toml").is_file());
    println!("\n{FRONTEND_CWD_SUCCESS}");
}

fn tool_program_directory() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/xtask-process-tools");
    fs::create_dir_all(&base).unwrap();
    let directory = tempfile::tempdir_in(base).unwrap();
    for name in ["config", ".local-tests"] {
        fs::create_dir(directory.path().join(name)).unwrap();
    }
    directory
}

fn write_tool_programs(root: &Path, relative: &str, fields: &[(&str, &str)]) {
    let verification = fields
        .iter()
        .map(|(key, value)| ((*key).to_owned(), toml::Value::String((*value).to_owned())))
        .collect::<toml::Table>();
    let document =
        toml::Table::from_iter([("verification".into(), toml::Value::Table(verification))]);
    fs::write(root.join(relative), toml::to_string(&document).unwrap()).unwrap();
}

#[cfg(windows)]
#[test]
fn owned_local_paths_accept_native_spelling_without_relaxing_file_or_link_guards() {
    use super::local_test_path::{LocalTestPathKind as Kind, validate_local_test_path};

    let temporary = tool_program_directory();
    let root = temporary.path().join("后端 空间");
    let managed = root.join(".local-tests").join("中文 目录");
    fs::create_dir_all(&managed).unwrap();
    let native_root = fs::canonicalize(&root).unwrap();
    let normal_root =
        std::path::PathBuf::from(native_root.to_str().unwrap().strip_prefix(r"\\?\").unwrap());
    let normal = normal_root
        .join(".local-tests")
        .join("中文 目录")
        .join("结果.bin");
    let native = native_root
        .join(".local-tests")
        .join("中文 目录")
        .join("结果.bin");
    let roots = [normal_root.as_path(), native_root.as_path()];
    let values = [normal.as_path(), native.as_path()];
    for root in roots {
        for value in values {
            assert_eq!(
                validate_local_test_path(value, root, Kind::OutputFile)
                    .unwrap()
                    .as_os_str(),
                value.as_os_str()
            );
            assert!(validate_local_test_path(value, root, Kind::ExistingFile).is_err());
            let directory = value.parent().unwrap();
            assert!(validate_local_test_path(directory, root, Kind::ExistingDirectory).is_ok());
            assert!(validate_local_test_path(directory, root, Kind::ExistingFile).is_err());
        }
        for bad in [
            temporary.path().join("outside.bin"),
            root.join(".local-tests-other").join("结果.bin"),
            root.join(".local-tests"),
            std::path::PathBuf::from("relative.bin"),
            std::path::PathBuf::from(format!(
                r"{}\..\结果.bin",
                native.parent().unwrap().display()
            )),
            std::path::PathBuf::from(format!(
                r"{}\.\结果.bin",
                native.parent().unwrap().display()
            )),
        ] {
            assert!(validate_local_test_path(&bad, root, Kind::OutputFile).is_err());
        }
    }
    assert!(!normal.exists());
    let target = root.join(".local-tests").join("真实 目标");
    fs::create_dir(&target).unwrap();
    let link = managed.join("link");
    std::os::windows::fs::symlink_dir(&target, &link).unwrap();
    for root in roots {
        for value in [
            normal_root
                .join(".local-tests")
                .join("中文 目录")
                .join("link")
                .join("结果.bin"),
            native_root
                .join(".local-tests")
                .join("中文 目录")
                .join("link")
                .join("结果.bin"),
        ] {
            let error = validate_local_test_path(&value, root, Kind::OutputFile).unwrap_err();
            assert!(error.contains("路径不能经过符号链接或 junction"));
            assert!(!target.join("结果.bin").exists());
        }
    }
    fs::remove_dir(&link).unwrap();
    fs::write(&normal, b"owned-original").unwrap();
    for root in roots {
        for value in values {
            let error = validate_local_test_path(value, root, Kind::OutputFile).unwrap_err();
            assert!(error.contains("输出文件已存在"));
            assert!(validate_local_test_path(value, root, Kind::ExistingFile).is_ok());
            assert_eq!(fs::read(value).unwrap(), b"owned-original");
        }
    }
}

#[cfg(windows)]
#[test]
fn owned_path_components_keep_unc_share_and_drive_boundaries_without_network_io() {
    use super::local_test_path::strip_local_test_prefix;

    let boundaries = [
        r"\\server\share\后端 根\.local-tests",
        r"\\?\UNC\server\share\后端 根\.local-tests",
    ];
    let values = [
        r"\\server\share\后端 根\.local-tests\中文 数据.bin",
        r"\\?\UNC\server\share\后端 根\.local-tests\中文 数据.bin",
    ];
    for boundary in boundaries {
        for value in values {
            assert_eq!(
                strip_local_test_prefix(Path::new(value), Path::new(boundary)),
                Some(Path::new("中文 数据.bin"))
            );
        }
    }
    for (value, boundary) in [
        (r"\\?\UNC\other\share\后端 根\.local-tests\x", boundaries[0]),
        (
            r"\\?\UNC\server\other\后端 根\.local-tests\x",
            boundaries[0],
        ),
        (r"E:\后端 根\.local-tests\x", r"D:\后端 根\.local-tests"),
        (r"\\.\D:\后端 根\.local-tests\x", r"D:\后端 根\.local-tests"),
        (
            r"\\server\share\后端 根\.local-tests-other\x",
            boundaries[1],
        ),
    ] {
        assert!(strip_local_test_prefix(Path::new(value), Path::new(boundary)).is_none());
    }
}

#[test]
fn python_runner_owns_utf8_arguments_and_environment() {
    assert_eq!(
        python_arguments(
            "python",
            &[
                "-B",
                "-W",
                "error",
                "-Xutf8=0",
                "-X",
                "UTF8=1",
                "script.py",
                "-X",
                "utf8",
            ],
        ),
        ["-X", "utf8", "-B", "-W", "error", "script.py", "-X", "utf8",]
    );
    assert_eq!(python_arguments("node", &["script.mjs"]), ["script.mjs"]);
    assert_eq!(
        python_environment(
            "python",
            &[
                ("pythonutf8", "0"),
                ("PythonIoEncoding", "ascii:strict"),
                ("RYFRAME_INPUT", "kept"),
            ],
        ),
        [
            ("RYFRAME_INPUT", "kept"),
            ("PYTHONUTF8", "1"),
            ("PYTHONIOENCODING", "utf-8"),
        ]
    );
}

#[test]
fn python_output_runner_applies_the_same_utf8_policy() {
    let output = command_output_with_env(
        std::path::Path::new("."),
        "python",
        &[
            "-c",
            "import json, os, sys; print(json.dumps([sys.flags.utf8_mode, os.environ.get('PYTHONUTF8'), os.environ.get('PYTHONIOENCODING')]))",
        ],
        &[
            ("pythonUtf8", "0"),
            ("pythonIoEncoding", "ascii:strict"),
        ],
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(output.trim()).unwrap(),
        serde_json::json!([1, "1", "utf-8"])
    );
}

const STDIN_CAPTURE_HELPER: &str = "process_tests::stdin_capture_fixture_helper";
const STDIN_CAPTURE_PREFIX: &str = "RYFRAME_STDIN_CAPTURE_RESPONSE:";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
enum StdinCaptureRequest {
    Echo { value: String },
    Fail { marker: std::path::PathBuf },
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StdinCaptureResponse {
    value: String,
    raw_in_environment: bool,
}

#[test]
#[ignore = "仅由 stdin 捕获父测试作为受控 Rust 子进程调用，非独立场景"]
fn stdin_capture_fixture_helper() {
    use super::local_test_path::{LocalTestPathKind, validate_local_test_path};
    use std::io::Write;

    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(!bytes.is_empty() && bytes.len() <= 16 * 1024);
    let raw = std::str::from_utf8(&bytes).expect("辅助请求必须是 UTF-8");
    let request: StdinCaptureRequest = serde_json::from_str(raw).expect("辅助请求必须是严格枚举");
    match request {
        StdinCaptureRequest::Echo { value } => {
            let response = StdinCaptureResponse {
                value,
                raw_in_environment: std::env::vars_os()
                    .any(|(_, value)| value.as_os_str() == std::ffi::OsStr::new(raw)),
            };
            println!(
                "\n{STDIN_CAPTURE_PREFIX}{}",
                serde_json::to_string(&response).unwrap()
            );
        }
        StdinCaptureRequest::Fail { marker } => {
            let kind = match fs::symlink_metadata(&marker) {
                Ok(_) => LocalTestPathKind::ExistingFile,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    LocalTestPathKind::OutputFile
                }
                Err(error) => panic!("无法核验受控 marker：{error}"),
            };
            validate_local_test_path(&marker, &super::workspace::root_dir(), kind).unwrap();
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(marker)
                .unwrap();
            file.write_all(b"x").unwrap();
            drop(file);
            std::process::exit(7);
        }
    }
}

#[test]
fn stdin_capture_transports_utf8_once_without_environment_and_preserves_failure() {
    let executable = std::env::current_exe().unwrap();
    assert!(executable.is_absolute());
    let args = [
        "--exact",
        STDIN_CAPTURE_HELPER,
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ];
    let value = "私有请求：原始 UTF-8 值";
    let request = serde_json::to_vec(&StdinCaptureRequest::Echo {
        value: value.into(),
    })
    .unwrap();
    let output = super::process::command_output_with_input(
        Path::new("."),
        executable.to_str().unwrap(),
        &args,
        &request,
    )
    .unwrap();
    let responses: Vec<_> = output
        .lines()
        .filter_map(|line| line.strip_prefix(STDIN_CAPTURE_PREFIX))
        .collect();
    assert_eq!(responses.len(), 1, "辅助进程必须只发布一行响应");
    assert_eq!(
        serde_json::from_str::<StdinCaptureResponse>(responses[0]).unwrap(),
        StdinCaptureResponse {
            value: value.into(),
            raw_in_environment: false
        }
    );

    let directory = tool_program_directory();
    let marker = directory.path().join("once.txt");
    let request = serde_json::to_vec(&StdinCaptureRequest::Fail {
        marker: marker.clone(),
    })
    .unwrap();
    let error = super::process::command_output_with_input(
        Path::new("."),
        executable.to_str().unwrap(),
        &args,
        &request,
    )
    .unwrap_err();
    assert_eq!(failure_exit_code(error.as_ref()), Some(7));
    assert_eq!(fs::read(marker).unwrap(), b"x");
}

#[test]
fn child_usage_and_task_failure_exit_codes_are_preserved() {
    for code in [2, 7] {
        let statement = format!("exit {code}");
        #[cfg(windows)]
        let (program, args) = ("cmd", vec!["/D", "/C", statement.as_str()]);
        #[cfg(unix)]
        let (program, args) = ("sh", vec!["-c", statement.as_str()]);
        let directory = std::path::Path::new(".");
        let error = run(directory, program, &args).unwrap_err();
        assert_eq!(failure_exit_code(error.as_ref()), Some(code));
        let error = command_output(directory, program, &args).unwrap_err();
        assert_eq!(failure_exit_code(error.as_ref()), Some(code));
    }
    assert_eq!(failure_exit_code(&std::io::Error::other("无法启动")), None);
}

#[test]
fn pnpm_commands_default_to_non_interactive_ci_mode() {
    let mut command = Command::new("pnpm");
    configure_pnpm_environment(&mut command, &[]);
    let ci = command
        .get_envs()
        .find(|(key, _)| *key == "CI")
        .and_then(|(_, value)| value)
        .and_then(|value| value.to_str());
    assert_eq!(ci, Some("true"));

    let mut overridden = Command::new("pnpm");
    configure_pnpm_environment(&mut overridden, &[("CI", "false")]);
    let ci = overridden
        .get_envs()
        .find(|(key, _)| *key == "CI")
        .and_then(|(_, value)| value)
        .and_then(|value| value.to_str());
    assert_eq!(ci, Some("false"));
}

#[test]
fn child_group_accepts_and_waits_for_child() {
    let group = ChildGroup::new().unwrap();
    #[cfg(windows)]
    let mut command = Command::new("cmd");
    #[cfg(windows)]
    command.args(["/C", "exit", "0"]);
    #[cfg(unix)]
    let mut command = Command::new("sh");
    #[cfg(unix)]
    command.args(["-c", "exit 0"]);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = group.spawn(command).unwrap().wait().unwrap();
    assert!(status.success());
}

#[test]
fn parallel_task_log_captures_child_output() {
    let path = std::env::temp_dir().join(format!(
        "ryframe-xtask-process-log-{}.txt",
        std::process::id()
    ));
    with_process_log("rustc-version", &path, || {
        run(std::path::Path::new("."), "rustc", &["--version"])
    })
    .unwrap();
    let log = fs::read_to_string(&path).unwrap();
    assert!(log.contains("rustc"));
    fs::remove_file(path).unwrap();
}
