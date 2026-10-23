#![cfg(unix)]
#![forbid(unsafe_code)]
#![allow(dead_code)]

use nix::{
    errno::Errno,
    sys::signal::{Signal, kill},
    unistd::Pid,
};

use std::{
    error::Error,
    fs,
    io::{Read, Seek, SeekFrom},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[path = "../src/process.rs"]
mod process;
#[path = "../src/workspace.rs"]
mod workspace;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const HELPER_NAME: &str = "process_group_descendant_helper";
const MAX_REQUEST_BYTES: u64 = 16 * 1024;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "role", rename_all = "kebab-case", deny_unknown_fields)]
enum HelperRequest {
    Parent {
        pid_file: std::path::PathBuf,
        ready_file: std::path::PathBuf,
    },
    Grandchild {
        ready_file: std::path::PathBuf,
    },
}

fn read_request(reader: impl Read) -> Result<HelperRequest> {
    let mut bytes = Vec::new();
    reader.take(MAX_REQUEST_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        return Err("POSIX 进程组测试请求超过 16 KiB 限制".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn helper_command(request: &HelperRequest, ignore_term: bool) -> Command {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.local-tests/process-unix");
    fs::create_dir_all(&root).expect("应能创建受管测试目录");
    let mut input = tempfile::tempfile_in(root).expect("应能创建测试请求文件");
    serde_json::to_writer(&mut input, request).expect("应能写入测试请求");
    input.seek(SeekFrom::Start(0)).expect("应能复位测试请求");
    let executable = std::env::current_exe().expect("应能定位测试程序");
    let mut command = if ignore_term {
        // 标准 shell 将 SIGTERM 设为忽略，再原位 exec 测试程序；忽略状态跨 exec 保留。
        // 可执行文件和参数通过位置参数传递，不把路径拼接进 shell 代码。
        let mut command = Command::new("sh");
        command
            .args(["-c", "trap '' TERM; exec \"$@\"", "ryframe-signal-fixture"])
            .arg(executable)
            .args([HELPER_NAME, "--exact", "--ignored", "--nocapture"]);
        command
    } else {
        let mut command = Command::new(executable);
        command.args([HELPER_NAME, "--exact", "--ignored", "--nocapture"]);
        command
    };
    command
        .stdin(Stdio::from(input))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

struct DescendantGuard(Option<i32>);

impl Drop for DescendantGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        }
    }
}

#[test]
#[ignore = "仅由 POSIX 进程组集成测试作为子进程调用"]
#[expect(
    clippy::zombie_processes,
    reason = "必须让直接父进程不等待后代便退出，才能验证独立进程组回收"
)]
fn process_group_descendant_helper() {
    match read_request(std::io::stdin().lock()).expect("应能读取受限测试请求") {
        HelperRequest::Parent {
            pid_file,
            ready_file,
        } => {
            let child = helper_command(&HelperRequest::Grandchild { ready_file }, true)
                .spawn()
                .expect("父辅助进程应能创建后代进程");
            fs::write(pid_file, child.id().to_string()).expect("应能记录后代进程 PID");
        }
        HelperRequest::Grandchild { ready_file } => {
            kill(Pid::this(), Signal::SIGTERM).expect("应能向后代自身发送真实 SIGTERM");
            fs::write(ready_file, b"ready").expect("应能记录后代已忽略 SIGTERM");
            thread::sleep(Duration::from_secs(60));
        }
    }
}

#[test]
fn helper_protocol_rejects_invalid_or_oversized_requests() {
    for invalid in [
        r#"{"role":"unknown"}"#,
        r#"{"role":"parent"}"#,
        r#"{"role":"grandchild","ready_file":"ready","extra":true}"#,
        r#"{"role":"grandchild","ready_file":"ready"} {}"#,
        "",
    ] {
        assert!(read_request(invalid.as_bytes()).is_err(), "{invalid}");
    }
    let oversized = vec![b' '; MAX_REQUEST_BYTES as usize + 1];
    assert!(
        read_request(oversized.as_slice())
            .unwrap_err()
            .to_string()
            .contains("16 KiB")
    );
}

#[test]
fn stopping_process_group_reclaims_descendant_after_direct_child_exits() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.local-tests/process-unix")
        .join(std::process::id().to_string());
    fs::create_dir_all(&root).expect("应能创建受管测试目录");
    let pid_file = root.join("descendant.pid");
    let ready_file = pid_file.with_extension("ready");
    let _ = fs::remove_file(&pid_file);
    let _ = fs::remove_file(&ready_file);

    let group = process::ChildGroup::new().expect("应能创建进程树工厂");
    let mut command = helper_command(
        &HelperRequest::Parent {
            pid_file: pid_file.clone(),
            ready_file: ready_file.clone(),
        },
        false,
    );
    let mut direct_child = group.spawn(command).expect("应能启动父辅助进程");
    wait_for_direct_child(&mut direct_child);
    wait_for_file(&pid_file);
    wait_for_file(&ready_file);

    let descendant_id = fs::read_to_string(&pid_file)
        .expect("应能读取后代 PID")
        .parse::<i32>()
        .expect("后代 PID 应为整数");
    let mut guard = DescendantGuard(Some(descendant_id));
    assert!(process_exists(descendant_id), "父退出后后代应仍在进程组中");
    kill(Pid::from_raw(descendant_id), Signal::SIGTERM).expect("应能发送真实温和终止信号");
    thread::sleep(Duration::from_millis(100));
    assert!(
        process_exists(descendant_id),
        "后代必须真实忽略 SIGTERM 才能验证整组强杀"
    );

    let stop_started = Instant::now();
    process::stop_child(&mut direct_child).expect("应能强制终止直接父已退出的独立进程组");
    assert!(
        stop_started.elapsed() < Duration::from_secs(1),
        "温和终止与强杀的总预算必须小于一秒"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_exists(descendant_id) {
        assert!(Instant::now() < deadline, "独立进程组未回收后代进程");
        thread::sleep(Duration::from_millis(25));
    }
    process::stop_child(&mut direct_child).expect("进程组已不存在时再次停止应成功");
    guard.0 = None;
    let _ = fs::remove_file(pid_file);
    let _ = fs::remove_file(ready_file);
}

fn wait_for_direct_child(child: &mut process::ManagedChild) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("应能查询父辅助进程状态") {
            assert!(status.success(), "父辅助进程执行失败");
            return;
        }
        assert!(Instant::now() < deadline, "父辅助进程未按时退出");
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_file(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.is_file() {
        assert!(Instant::now() < deadline, "未按时生成后代 PID 文件");
        thread::sleep(Duration::from_millis(25));
    }
}

fn process_exists(pid: i32) -> bool {
    !matches!(kill(Pid::from_raw(pid), None), Err(Errno::ESRCH))
}
