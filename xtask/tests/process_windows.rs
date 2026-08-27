#![cfg(windows)]
#![allow(dead_code)]

use std::{
    error::Error,
    fs,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    },
};

#[path = "../src/process.rs"]
mod process;
#[path = "../src/workspace.rs"]
mod workspace;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const HELPER_NAME: &str = "job_descendant_helper";
const HELPER_ROLE: &str = "RYFRAME_XTASK_JOB_TEST_ROLE";
const PID_FILE: &str = "RYFRAME_XTASK_JOB_TEST_PID_FILE";

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[test]
#[ignore = "仅由 Windows Job Object 集成测试作为子进程调用"]
#[expect(
    clippy::zombie_processes,
    reason = "必须让直接父进程不等待后代便退出，才能验证 Job Object 独立回收后代"
)]
fn job_descendant_helper() {
    match std::env::var(HELPER_ROLE).as_deref() {
        Ok("parent") => {
            let child = Command::new(std::env::current_exe().expect("应能定位测试程序"))
                .args([HELPER_NAME, "--exact", "--ignored", "--nocapture"])
                .env(HELPER_ROLE, "grandchild")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("父辅助进程应能创建后代进程");
            fs::write(
                std::env::var_os(PID_FILE).expect("应提供后代 PID 文件"),
                child.id().to_string(),
            )
            .expect("应能记录后代进程 PID");
        }
        Ok("grandchild") => thread::sleep(Duration::from_secs(60)),
        role => panic!("未知的 Windows Job 测试角色：{role:?}"),
    }
}

#[test]
fn stopping_command_tree_reclaims_descendant_after_direct_child_exits() {
    let pid_file = std::env::temp_dir().join(format!(
        "ryframe-xtask-job-descendant-{}.pid",
        std::process::id()
    ));
    let _ = fs::remove_file(&pid_file);

    let group = process::ChildGroup::new().expect("应能创建命令进程树工厂");
    let mut command = Command::new(std::env::current_exe().expect("应能定位测试程序"));
    command
        .args([HELPER_NAME, "--exact", "--ignored", "--nocapture"])
        .env(HELPER_ROLE, "parent")
        .env(PID_FILE, &pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut direct_child = group.spawn(&mut command).expect("应能启动父辅助进程");
    let deadline = Instant::now() + Duration::from_secs(10);
    let direct_status = loop {
        if let Some(status) = direct_child.try_wait().expect("应能查询父辅助进程状态") {
            break status;
        }
        assert!(Instant::now() < deadline, "父辅助进程未按时退出");
        thread::sleep(Duration::from_millis(25));
    };
    assert!(direct_status.success(), "父辅助进程执行失败");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !pid_file.is_file() {
        assert!(Instant::now() < deadline, "未按时生成后代 PID 文件");
        thread::sleep(Duration::from_millis(25));
    }
    let descendant_id = fs::read_to_string(&pid_file)
        .expect("应能读取后代 PID")
        .parse::<u32>()
        .expect("后代 PID 应为整数");
    let descendant = OwnedHandle(unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            descendant_id,
        )
    });
    assert!(!descendant.0.is_null(), "后代进程应仍在运行");
    assert_eq!(
        unsafe { WaitForSingleObject(descendant.0, 0) },
        WAIT_TIMEOUT,
        "父辅助进程退出后，后代仍应由命令级 Job Object 持有"
    );

    process::stop_child(&mut direct_child).expect("应能终止直接父已退出的命令进程树");
    assert_eq!(
        unsafe { WaitForSingleObject(descendant.0, 5_000) },
        WAIT_OBJECT_0,
        "停止命令级 Job Object 应回收已脱离直接父进程的后代"
    );
    let _ = fs::remove_file(pid_file);
}
