use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use super::{check::run_parallel_tasks, process, process_liveness};

static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

const HELPER: &str = "check_cancellation_tests::parallel_gate_descendant_helper";
const MAX_REQUEST_BYTES: u64 = 16 * 1024;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperRequest {
    role: HelperRole,
    tree: std::path::PathBuf,
    parent_exit: bool,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum HelperRole {
    Parent,
    Child,
    Grandchild,
}

impl HelperRole {
    fn label(self) -> &'static str {
        match self {
            Self::Parent => "parent",
            Self::Child => "child",
            Self::Grandchild => "grandchild",
        }
    }

    fn next(self) -> Option<Self> {
        match self {
            Self::Parent => Some(Self::Child),
            Self::Child => Some(Self::Grandchild),
            Self::Grandchild => None,
        }
    }
}

fn read_helper_request(reader: impl Read) -> super::Result<HelperRequest> {
    let mut bytes = Vec::new();
    reader.take(MAX_REQUEST_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        return Err("并行门禁测试请求超过 16 KiB 限制".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[test]
#[ignore = "仅由并行完整门禁回收测试作为子进程调用"]
fn parallel_gate_descendant_helper() {
    let request = read_helper_request(std::io::stdin().lock()).expect("应能读取受限辅助请求");
    let role = request.role.label();
    let tree = request.tree;
    fs::write(
        tree.join(format!("{role}.pid")),
        std::process::id().to_string(),
    )
    .expect("应能记录辅助进程 PID");
    let next = request.role.next();
    if let Some(next) = next {
        let mut child = helper_command(&HelperRequest {
            role: next,
            tree: tree.clone(),
            parent_exit: request.parent_exit,
        })
        .spawn()
        .expect("应能启动下一层辅助进程");
        if matches!(request.role, HelperRole::Parent) && request.parent_exit {
            wait_for_file(&tree.join("ready"));
            // fixture 主线程先退出，后台等待线程随宿主结束，留下待 Job/进程组回收的后代。
            thread::spawn(move || {
                let _ = child.wait();
            });
            return;
        }
        let _ = child.wait();
    } else {
        fs::write(tree.join("ready"), b"ready").expect("应能记录三层进程树就绪");
        thread::sleep(Duration::from_secs(60));
    }
}

#[test]
fn parallel_helper_protocol_rejects_unknown_missing_trailing_and_oversized_requests() {
    for invalid in [
        r#"{"role":"unknown","tree":"tree","parent_exit":false}"#,
        r#"{"role":"parent","tree":"tree"}"#,
        r#"{"role":"parent","tree":"tree","parent_exit":false,"extra":true}"#,
        r#"{"role":"parent","tree":"tree","parent_exit":false} {}"#,
        "",
    ] {
        assert!(
            read_helper_request(invalid.as_bytes()).is_err(),
            "{invalid}"
        );
    }
    let oversized = vec![b' '; MAX_REQUEST_BYTES as usize + 1];
    assert!(
        read_helper_request(oversized.as_slice())
            .unwrap_err()
            .to_string()
            .contains("16 KiB")
    );
}

#[test]
fn parallel_gate_cancels_sibling_tree_and_preserves_first_exit_code() {
    assert_parallel_cancellation("resource-workspace", "backend-workspace", 37, true);
    assert_parallel_cancellation("backend-workspace", "resource-workspace", 38, false);
}

#[test]
fn completed_parent_reaps_descendants_before_returning_and_keeps_unrelated_tree() {
    for capture in [false, true] {
        let root = test_directory();
        let unrelated_root = root.join("无关进程");
        fs::create_dir(&unrelated_root).unwrap();
        let mut unrelated_command = helper_command(&HelperRequest {
            role: HelperRole::Grandchild,
            tree: unrelated_root.clone(),
            parent_exit: false,
        });
        unrelated_command
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut unrelated = process::ChildGroup::new()
            .unwrap()
            .spawn(unrelated_command)
            .unwrap();
        wait_for_file(&unrelated_root.join("ready"));
        let executable = std::env::current_exe().unwrap();
        let args = [HELPER, "--exact", "--ignored", "--nocapture"];
        let input = helper_payload(&HelperRequest {
            role: HelperRole::Parent,
            tree: root.clone(),
            parent_exit: true,
        });
        let started = Instant::now();
        run_parallel_tasks(
            &root,
            "orphan-parent",
            || {
                if capture {
                    let output = process::command_output_with_input_timeout(
                        &root,
                        executable.to_str().unwrap(),
                        &args,
                        &[],
                        Some(&input),
                        Duration::from_secs(70),
                    )?;
                    if output.status.success() {
                        Ok(())
                    } else {
                        Err(process::PreservedFailure::new(
                            "并行门禁辅助进程失败".to_owned(),
                            output.status.code(),
                        )
                        .into())
                    }
                } else {
                    process::run_with_stdin(
                        &root,
                        executable.to_str().unwrap(),
                        &args,
                        stdin_file(&root, &input),
                    )
                }
            },
            "peer",
            || Ok(()),
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        // 不额外等待：返回前就必须完成整树回收。
        for role in ["parent", "child", "grandchild"] {
            let pid = fs::read_to_string(root.join(format!("{role}.pid")))
                .unwrap()
                .parse()
                .unwrap();
            assert!(
                !process_liveness::process_is_running(pid),
                "返回后仍有后代 {role}"
            );
        }
        assert!(unrelated.try_wait().unwrap().is_none());
        process::stop_child(&mut unrelated).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn panicking_branch_cancels_and_joins_its_sibling() {
    let root = test_directory();
    let executable = std::env::current_exe().unwrap();
    let error = run_parallel_tasks(
        &root,
        "long",
        || run_long_tree(&root, executable.to_str().unwrap(), true),
        "panic",
        || {
            wait_for_file(&root.join("ready"));
            panic!("并行回收 panic 证据");
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("并行回收 panic 证据"));
    for role in ["parent", "child", "grandchild"] {
        let pid = fs::read_to_string(root.join(format!("{role}.pid")))
            .unwrap()
            .parse()
            .unwrap();
        wait_for_exit(pid);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancellation_rejects_followup_command_before_spawn() {
    let root = test_directory();
    let cancellation = process::ProcessCancellation::new();
    cancellation.request();
    let error = process::with_process_cancellation(&cancellation, || {
        process::run(&root, "ryframe-nonexistent-command", &[])
    })
    .unwrap_err();
    assert!(process::is_process_cancellation(error.as_ref()));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn simultaneous_failures_keep_both_reports_and_the_first_exit_code() {
    let root = test_directory();
    let barrier = std::sync::Barrier::new(2);
    let failure = |label: &str, code| {
        barrier.wait();
        Err(process::PreservedFailure::new(label.to_owned(), Some(code)).into())
    };
    let error = run_parallel_tasks(
        &root,
        "left",
        || failure("左侧原始失败", 37),
        "right",
        || failure("右侧回收失败", 38),
    )
    .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("左侧原始失败"));
    assert!(text.contains("右侧回收失败"));
    let expected = if text.starts_with("并行任务 left") {
        37
    } else {
        38
    };
    assert_eq!(process::failure_exit_code(error.as_ref()), Some(expected));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failure_formatting_panic_cannot_be_reported_as_parallel_success() {
    #[derive(Debug)]
    struct BadDiagnostic;
    impl std::fmt::Display for BadDiagnostic {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("失败诊断格式化 panic");
        }
    }
    impl std::error::Error for BadDiagnostic {}
    let root = test_directory();
    let error = run_parallel_tasks(
        &root,
        "bad-diagnostic",
        || Err(BadDiagnostic.into()),
        "peer",
        || Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("失败诊断格式化 panic"));
    fs::remove_dir_all(root).unwrap();
}

fn test_directory() -> std::path::PathBuf {
    let run = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "ryframe xtask 并行回收-{}-{run}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

fn helper_command(request: &HelperRequest) -> Command {
    let input = helper_payload(request);
    let file = stdin_file(&request.tree, &input);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([HELPER, "--exact", "--ignored", "--nocapture"])
        .stdin(file);
    command
}

fn helper_payload(request: &HelperRequest) -> Vec<u8> {
    serde_json::to_vec(request).expect("应能序列化受限辅助请求")
}

fn stdin_file(root: &Path, input: &[u8]) -> Stdio {
    let mut file = tempfile::tempfile_in(root).expect("应能创建受管请求文件");
    std::io::Write::write_all(&mut file, input).expect("应能写入受管请求");
    file.seek(SeekFrom::Start(0)).expect("应能复位受管请求");
    Stdio::from(file)
}

fn assert_parallel_cancellation(
    long_label: &str,
    failure_label: &str,
    failure_code: i32,
    capture_output: bool,
) {
    let run = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "ryframe xtask 并行回收-{}-{run}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let executable = std::env::current_exe().unwrap();
    let executable = executable.to_str().unwrap();
    let started = Instant::now();
    let error = run_parallel_tasks(
        &root,
        long_label,
        || run_long_tree(&root, executable, capture_output),
        failure_label,
        || fail_after_tree_ready(&root, failure_code),
    )
    .unwrap_err();

    assert_eq!(
        process::failure_exit_code(error.as_ref()),
        Some(failure_code)
    );
    assert!(error.to_string().contains(failure_label));
    assert!(error.to_string().contains(&format!("exit {failure_code}")));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "兄弟任务失败后不应等待长驻进程自然退出"
    );
    for role in ["parent", "child", "grandchild"] {
        let pid = fs::read_to_string(root.join(format!("{role}.pid")))
            .unwrap()
            .parse()
            .unwrap();
        wait_for_exit(pid);
    }
    fs::remove_dir_all(root).unwrap();
}

fn run_long_tree(root: &Path, executable: &str, capture: bool) -> super::Result<()> {
    let args = [HELPER, "--exact", "--ignored", "--nocapture"];
    let input = helper_payload(&HelperRequest {
        role: HelperRole::Parent,
        tree: root.to_path_buf(),
        parent_exit: false,
    });
    if capture {
        let output = process::command_output_with_input_timeout(
            root,
            executable,
            &args,
            &[],
            Some(&input),
            Duration::from_secs(70),
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(process::PreservedFailure::new(
                "并行门禁辅助进程失败".to_owned(),
                output.status.code(),
            )
            .into())
        }
    } else {
        process::run_with_stdin(root, executable, &args, stdin_file(root, &input))
    }
}

fn fail_after_tree_ready(root: &Path, code: i32) -> super::Result<()> {
    wait_for_file(&root.join("ready"));
    let statement = format!("exit {code}");
    #[cfg(windows)]
    let (program, args) = ("cmd", ["/D", "/C", statement.as_str()]);
    #[cfg(unix)]
    let (program, args) = ("sh", ["-c", statement.as_str()]);
    process::run(root, program, &args)
}

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.is_file() {
        assert!(Instant::now() < deadline, "三层进程树未按时就绪");
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_exit(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_liveness::process_is_running(pid) {
        assert!(Instant::now() < deadline, "并行任务后代进程 {pid} 未被回收");
        thread::sleep(Duration::from_millis(10));
    }
}
