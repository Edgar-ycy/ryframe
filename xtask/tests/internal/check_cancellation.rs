use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use super::{check::run_parallel_tasks, process};

static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

const HELPER: &str = "check_cancellation_tests::parallel_gate_descendant_helper";
const ROLE: &str = "RYFRAME_XTASK_PARALLEL_TEST_ROLE";
const TREE_DIR: &str = "RYFRAME_XTASK_PARALLEL_TEST_TREE_DIR";

#[test]
#[ignore = "仅由并行完整门禁回收测试作为子进程调用"]
fn parallel_gate_descendant_helper() {
    let role = std::env::var(ROLE).expect("辅助进程应具有角色");
    let tree =
        std::path::PathBuf::from(std::env::var_os(TREE_DIR).expect("辅助进程应具有记录目录"));
    fs::write(
        tree.join(format!("{role}.pid")),
        std::process::id().to_string(),
    )
    .expect("应能记录辅助进程 PID");
    let next = match role.as_str() {
        "parent" => Some("child"),
        "child" => Some("grandchild"),
        "grandchild" => None,
        _ => panic!("未知的并行回收辅助角色：{role}"),
    };
    if let Some(next) = next {
        let mut child = Command::new(std::env::current_exe().expect("应能定位测试程序"))
            .args([HELPER, "--exact", "--ignored", "--nocapture"])
            .env(ROLE, next)
            .env(TREE_DIR, &tree)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("应能启动下一层辅助进程");
        let _ = child.wait();
    } else {
        fs::write(tree.join("ready"), b"ready").expect("应能记录三层进程树就绪");
        thread::sleep(Duration::from_secs(60));
    }
}

#[test]
fn parallel_gate_cancels_sibling_tree_and_preserves_first_exit_code() {
    assert_parallel_cancellation("resource-workspace", "backend-workspace", 37, true);
    assert_parallel_cancellation("backend-workspace", "resource-workspace", 38, false);
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
    let root_text = root.to_str().unwrap();
    let started = Instant::now();
    let error = run_parallel_tasks(
        &root,
        long_label,
        || run_long_tree(&root, executable, root_text, capture_output),
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

fn run_long_tree(
    root: &Path,
    executable: &str,
    root_text: &str,
    capture: bool,
) -> super::Result<()> {
    let args = [HELPER, "--exact", "--ignored", "--nocapture"];
    let environment = [(ROLE, "parent"), (TREE_DIR, root_text)];
    if capture {
        process::command_output_with_env(root, executable, &args, &environment).map(|_| ())
    } else {
        process::run_with_env(root, executable, &args, &environment)
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
    while process::process_is_running(pid) {
        assert!(Instant::now() < deadline, "并行任务后代进程 {pid} 未被回收");
        thread::sleep(Duration::from_millis(10));
    }
}
