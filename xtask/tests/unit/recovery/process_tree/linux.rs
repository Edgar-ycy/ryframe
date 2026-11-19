use super::*;
use crate::process::{ChildGroup, ManagedChild};
use std::{io::Write, process::Stdio};

fn controlled_member() -> (ManagedChild, Box<dyn Write + Send>, OwnedFd) {
    let mut command = Command::new("/bin/cat");
    command.stdin(Stdio::piped()).stdout(Stdio::null());
    let mut child = ChildGroup::new().unwrap().spawn(command).unwrap();
    let input = child.take_stdin_writer().unwrap().unwrap();
    let pid = Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
    let fd = pidfd_open(pid, PidfdFlags::empty()).unwrap();
    (child, input, fd)
}

fn wait_for_exit(fd: &OwnedFd) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !exited(fd).unwrap() {
        assert!(Instant::now() < deadline, "原子进程必须在本用例内退出");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn linux_full_stack_member_samples_the_original_live_child() {
    let (mut child, input, fd) = controlled_member();
    let pid = child.id();
    let observed = observe_process_group_member(pid, pid).unwrap().unwrap();
    assert_eq!(
        Some(observed.identity()),
        common::process_identity(pid).unwrap().as_ref(),
        "全栈收据必须保留原身份格式与字段"
    );
    assert!(observed.verify_group(pid).unwrap());
    assert!(observed.verify_group(pid + 1).is_err());
    assert!(observed_member(&fd, pid, pid + 1, |_| panic!("错组身份不得继续读取映像")).is_err());
    let denied = observed_member(&fd, pid, pid, |_| {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    })
    .unwrap_err();
    assert_eq!(
        denied.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::PermissionDenied
    );
    let mut images = ["one", "two"].into_iter();
    assert!(observed_member(&fd, pid, pid, |_| Ok(PathBuf::from(images.next().unwrap()))).is_err());
    assert!(!exited(&fd).unwrap());
    drop(input);
    wait_for_exit(&fd);
    assert!(observed.exited().unwrap());
    assert!(!observed.verify_group(pid).unwrap());
    assert!(child.wait().unwrap().success());
}

#[test]
fn linux_full_stack_member_skips_a_child_exiting_during_the_image_read() {
    let (mut child, input, fd) = controlled_member();
    let pid = child.id();
    let mut input = Some(input);
    let observation = observed_member(&fd, pid, pid, |path| {
        assert!(!exited(&fd).unwrap());
        drop(input.take().unwrap());
        wait_for_exit(&fd);
        assert!(
            path.with_file_name("stat").is_file(),
            "尚未回收的子进程仍有 stat"
        );
        let observed = fs::read_link(path);
        assert_eq!(
            observed.as_ref().unwrap_err().kind(),
            io::ErrorKind::NotFound,
            "原映像读取必须遇到真实的进程退出竞态"
        );
        observed
    })
    .unwrap();
    assert!(observation.is_none());
    assert!(child.wait().unwrap().success());
}

#[test]
fn linux_full_stack_member_ignores_changed_second_image_after_original_exit() {
    let (mut child, input, fd) = controlled_member();
    let pid = child.id();
    let mut input = Some(input);
    let mut reads = 0;
    let observation = observed_member(&fd, pid, pid, |path| {
        reads += 1;
        if reads == 1 {
            return fs::read_link(path);
        }
        assert_eq!(reads, 2);
        drop(input.take().unwrap());
        wait_for_exit(&fd);
        Ok(PathBuf::from("/different-executable"))
    })
    .unwrap();
    assert_eq!(reads, 2);
    assert!(observation.is_none());
    assert!(child.wait().unwrap().success());
}
