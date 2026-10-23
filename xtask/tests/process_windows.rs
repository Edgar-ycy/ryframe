#![cfg(windows)]
#![forbid(unsafe_code)]
#![allow(dead_code)]

use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use winsafe::{HPROCESS, co, prelude::kernel_Hprocess};

#[path = "../src/process.rs"]
mod process;
#[path = "support/process_liveness.rs"]
mod process_liveness;
#[path = "../src/workspace.rs"]
mod workspace;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const HELPER_NAME: &str = "job_descendant_helper";
const MAX_REQUEST_BYTES: u64 = 16 * 1024;
const HELD_MEMORY_BYTES: usize = 32 * 1024 * 1024;

static RUN_ID: AtomicU64 = AtomicU64::new(0);

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "role", rename_all = "kebab-case", deny_unknown_fields)]
enum HelperRequest {
    Parent { files: TestFiles },
    Grandchild { tree_ready: std::path::PathBuf },
    Controller { files: TestFiles },
    StdoutExit {},
}

fn read_request(reader: impl Read) -> Result<HelperRequest> {
    let mut bytes = Vec::new();
    reader.take(MAX_REQUEST_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        return Err("Windows Job 测试请求超过 16 KiB 限制".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn helper_command(request: &HelperRequest) -> Command {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.local-tests/process-windows");
    fs::create_dir_all(&root).expect("应能创建受管测试目录");
    // 使用匿名受管文件提供 stdin，避免引入 runtime 或修改产品进程接口。
    let mut input = tempfile::tempfile_in(root).expect("应能创建测试请求文件");
    serde_json::to_writer(&mut input, request).expect("应能写入测试请求");
    input.seek(SeekFrom::Start(0)).expect("应能复位测试请求");
    let mut command = Command::new(std::env::current_exe().expect("应能定位测试程序"));
    command
        .args([HELPER_NAME, "--exact", "--ignored", "--nocapture"])
        .stdin(Stdio::from(input))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[test]
fn helper_protocol_rejects_invalid_or_oversized_requests() {
    for invalid in [
        r#"{"role":"unknown"}"#,
        r#"{"role":"stdout-exit","extra":true}"#,
        r#"{"role":"parent"}"#,
        r#"{"role":"grandchild","tree_ready":"ready","extra":true}"#,
        r#"{"role":"stdout-exit"} {}"#,
        "",
    ] {
        assert!(read_request(invalid.as_bytes()).is_err(), "{invalid}");
    }
    let oversized = vec![b' '; MAX_REQUEST_BYTES as usize + 1];
    let error = read_request(oversized.as_slice())
        .err()
        .expect("应拒绝超长请求");
    assert!(error.to_string().contains("16 KiB"));
    assert!(matches!(
        read_request(br#"{"role":"stdout-exit"}"#.as_slice()).unwrap(),
        HelperRequest::StdoutExit {}
    ));
}

#[test]
#[ignore = "仅由 Windows Job Object 集成测试作为子进程调用"]
#[expect(
    clippy::zombie_processes,
    reason = "必须让直接父进程不等待后代便退出，才能验证 Job Object 独立回收后代"
)]
fn job_descendant_helper() {
    match read_request(std::io::stdin().lock()).expect("应能读取受限测试请求") {
        HelperRequest::Parent { files } => {
            let child = helper_command(&HelperRequest::Grandchild {
                tree_ready: files.tree_ready.clone(),
            })
            .spawn()
            .expect("父辅助进程应能创建后代进程");
            fs::write(&files.descendant_pid, child.id().to_string()).expect("应能记录后代进程 PID");
        }
        HelperRequest::Grandchild { tree_ready } => hold_committed_memory(&tree_ready),
        HelperRequest::StdoutExit {} => {
            println!("{{\"reason\":\"compiler-artifact\",\"target\":{{\"name\":\"fixture\"}}}}");
            std::process::exit(37);
        }
        HelperRequest::Controller { files } => run_abrupt_controller(files),
    }
}

#[test]
fn stopping_command_tree_reclaims_descendant_after_direct_child_exits() {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "普通 xtask 进程管理路径不得要求 Tokio runtime"
    );
    let files = TestFiles::new("stop");
    let group = process::ChildGroup::new().expect("应能创建命令进程树工厂");
    let mut direct_child = group
        .spawn(tree_parent_command(&files))
        .expect("应能启动父辅助进程");
    let direct_status = wait_for_child(&mut direct_child);
    assert!(direct_status.success(), "父辅助进程执行失败");
    wait_for_file(&files.tree_ready, "后代进程未完成内存准备");

    let descendant_id = read_pid(&files.descendant_pid);
    let descendant = open_process(descendant_id);
    assert!(
        process_liveness::process_is_running(descendant_id),
        "后代进程应仍在运行"
    );
    assert_eq!(
        descendant
            .WaitForSingleObject(Some(0))
            .expect("应能查询后代进程状态"),
        co::WAIT::TIMEOUT,
        "父辅助进程退出后，后代仍应由命令级 Job Object 持有"
    );
    assert_eq!(
        direct_child
            .active_process_count()
            .expect("应能查询 Job Object 活跃进程数"),
        Some(1),
        "启动后立即派生的后代必须位于同一 Job Object"
    );
    assert!(
        direct_child
            .job_stats()
            .expect("应能读取 Job Object 峰值内存")
            .peak_memory_bytes
            .is_some_and(|bytes| bytes >= HELD_MEMORY_BYTES as u64),
        "PeakJobMemoryUsed 应覆盖后代已提交内存"
    );

    process::stop_child(&mut direct_child).expect("应能终止直接父已退出的命令进程树");
    assert_eq!(
        direct_child
            .active_process_count()
            .expect("应能查询 Job Object 活跃进程数"),
        Some(0),
        "停止命令返回时 Job Object 不得保留孤儿进程"
    );
    assert_eq!(
        descendant
            .WaitForSingleObject(Some(5_000))
            .expect("应能等待后代进程退出"),
        co::WAIT::OBJECT_0,
        "停止命令级 Job Object 应回收已脱离直接父进程的后代"
    );
    assert!(
        !process_liveness::process_is_running(descendant_id),
        "仍持有句柄的已退出后代不得被判为存活"
    );
    files.cleanup();
}

#[test]
fn sync_stdout_reader_and_original_exit_code_work_without_runtime() {
    assert!(tokio::runtime::Handle::try_current().is_err());
    let group = process::ChildGroup::new().expect("应能创建命令进程树工厂");
    let mut command = helper_command(&HelperRequest::StdoutExit {});
    command.stdout(Stdio::piped());
    let mut child = group.spawn(command).expect("无 runtime 时仍应能启动命令");
    let mut stdout = child
        .take_stdout_reader()
        .expect("应能安全转换 stdout")
        .expect("应建立 stdout pipe");
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout
            .read_to_string(&mut output)
            .expect("应能同步读取 stdout");
        output
    });
    let status = child.wait().expect("应能同步等待直接子进程");
    assert_eq!(status.code(), Some(37), "必须保留原始退出码");
    let output = reader.join().expect("stdout 读取线程不应 panic");
    let json = output
        .lines()
        .find(|line| line.starts_with('{'))
        .expect("stdout 应包含 Cargo 风格 JSON 行");
    let value: serde_json::Value = serde_json::from_str(json).expect("输出应为 JSON");
    assert_eq!(value["reason"], "compiler-artifact");
    assert_eq!(child.active_process_count().unwrap(), Some(0));
}

#[test]
fn owner_exit_closes_job_and_reclaims_descendant() {
    let files = TestFiles::new("owner-exit");
    let mut controller = helper_command(&HelperRequest::Controller {
        files: files.clone(),
    });
    let mut controller = controller.spawn().expect("应能启动外层控制器");
    wait_for_file(&files.controller_ready, "控制器未就绪");
    let descendant = open_process(read_pid(&files.descendant_pid));
    fs::write(&files.controller_ack, b"ack").expect("应能确认已持有后代句柄");
    let status = controller.wait().expect("应能等待控制器退出");
    assert_eq!(status.code(), Some(23), "控制器应跳过 Rust 析构直接退出");
    assert_eq!(
        descendant
            .WaitForSingleObject(Some(5_000))
            .expect("应能等待后代退出"),
        co::WAIT::OBJECT_0,
        "拥有进程退出关闭 Job 句柄后，内核必须回收整个树"
    );
    files.cleanup();
}

#[test]
fn process_liveness_distinguishes_current_process_from_unopenable_pid() {
    assert!(process_liveness::process_is_running(std::process::id()));
    assert!(!process_liveness::process_is_running(0));
}

fn run_abrupt_controller(files: TestFiles) -> ! {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "控制器原型不得依赖 Tokio runtime"
    );
    let group = process::ChildGroup::new().expect("应能创建命令进程树工厂");
    let _child = group
        .spawn(tree_parent_command(&files))
        .expect("无 runtime 时仍应能启动受控进程");
    wait_for_file(&files.tree_ready, "受控进程树未就绪");
    fs::write(&files.controller_ready, b"ready").expect("应能记录控制器就绪");
    wait_for_file(&files.controller_ack, "外层测试未确认后代句柄");
    std::process::exit(23)
}

fn tree_parent_command(files: &TestFiles) -> Command {
    helper_command(&HelperRequest::Parent {
        files: files.clone(),
    })
}

fn hold_committed_memory(tree_ready: &std::path::Path) {
    let memory = vec![0xA5_u8; HELD_MEMORY_BYTES];
    fs::write(tree_ready, b"ready").expect("应能记录进程树就绪");
    std::hint::black_box(&memory);
    thread::sleep(Duration::from_secs(60));
}

fn wait_for_child(child: &mut process::ManagedChild) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("应能查询父辅助进程状态") {
            return status;
        }
        assert!(Instant::now() < deadline, "父辅助进程未按时退出");
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_file(path: &std::path::Path, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.is_file() {
        assert!(Instant::now() < deadline, "{message}: {}", path.display());
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_pid(path: &std::path::Path) -> u32 {
    fs::read_to_string(path)
        .expect("应能读取后代 PID")
        .parse()
        .expect("后代 PID 应为整数")
}

fn open_process(pid: u32) -> winsafe::guard::CloseHandleGuard<HPROCESS> {
    HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    )
    .expect("应能持有后代进程的真实句柄")
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TestFiles {
    descendant_pid: std::path::PathBuf,
    tree_ready: std::path::PathBuf,
    controller_ready: std::path::PathBuf,
    controller_ack: std::path::PathBuf,
}

impl TestFiles {
    fn new(label: &str) -> Self {
        let id = RUN_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.local-tests/process-windows")
            .join(format!("{label}-{}-{id}", std::process::id()));
        fs::create_dir_all(&root).expect("应能创建测试目录");
        Self::under(root)
    }

    fn under(root: std::path::PathBuf) -> Self {
        Self {
            descendant_pid: root.join("descendant.pid"),
            tree_ready: root.join("tree.ready"),
            controller_ready: root.join("controller.ready"),
            controller_ack: root.join("controller.ack"),
        }
    }

    fn cleanup(&self) {
        let root = self
            .descendant_pid
            .parent()
            .expect("测试文件应位于临时目录");
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }
}
