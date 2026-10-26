use super::*;

#[test]
fn stalled_reader_returns_failure_and_preserves_partial_output_without_completion() {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("source-reader-teardown-")
        .tempdir_in(local)
        .unwrap();
    std::fs::write(directory.path().join("source-producer-ready.json"), b"{}\n").unwrap();
    let (sender, receiver) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let (done, observed) = mpsc::channel();
    let reader = thread::spawn(move || {
        let _ = blocked.recv();
    });
    let watchdog = thread::spawn(move || {
        // 即使回归为无界 join，也在守卫窗口后释放测试线程，不遗留挂死进程。
        let completed_without_release = observed.recv_timeout(Duration::from_secs(3)).is_ok();
        let _ = release.send(());
        completed_without_release
    });
    let readers = Readers {
        receiver,
        threads: vec![reader],
        stdout: Arc::new(Mutex::new(b"{\"partial\":".to_vec())),
        stderr: Arc::new(Mutex::new(b"private-partial-diagnostic".to_vec())),
    };
    let error = readers
        .finish(directory.path(), true)
        .unwrap_err()
        .to_string();
    done.send(()).unwrap();
    assert!(watchdog.join().unwrap(), "回收必须在读取线程仍被阻塞时返回");
    assert_eq!(
        std::fs::read(directory.path().join("source-producer.stdout.json")).unwrap(),
        b"{\"partial\":"
    );
    assert_eq!(
        std::fs::read(directory.path().join("source-producer.stderr.log")).unwrap(),
        b"private-partial-diagnostic"
    );
    assert!(
        !directory
            .path()
            .join("source-producer-completion.json")
            .exists()
    );
    assert!(error.contains("禁止重放"));
    assert!(!error.contains("private-partial-diagnostic"));
    drop(sender);
}
