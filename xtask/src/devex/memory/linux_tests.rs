use super::*;
use std::io::{Read, Seek, SeekFrom};

const INPUT_LIMIT: usize = 65536;
const CHILD_TIMEOUT: Duration = Duration::from_secs(10);

#[test]
#[ignore = "仅由 stdin 透传测试作为子进程调用"]
fn stdin_target() {
    if let Some(result) = run_trampoline_if_requested() {
        result.unwrap();
        unreachable!("成功的 exec 不会返回");
    }
    for (key, _) in env::vars_os() {
        let key = key.to_string_lossy().to_ascii_uppercase();
        assert!(!key.starts_with("RYFRAME_"));
        assert!(!key.starts_with("VITE_"));
        assert!(!key.starts_with("SNOWFLAKE_"));
        assert!(!key.starts_with("APP_") || key == "APP_ENV");
    }
    assert_eq!(env::var("APP_ENV").unwrap(), "test");
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((INPUT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .unwrap();
    if bytes.is_empty() {
        std::process::exit(2);
    }
    if bytes.len() > INPUT_LIMIT {
        std::process::exit(3);
    }
    std::io::stdout().write_all(&bytes).unwrap();
    std::io::stderr().write_all(b"stdin-target-stderr").unwrap();
}

fn invocation(input: Stdio) -> Command {
    let executable = env::current_exe().unwrap();
    let mut command = Command::new(&executable);
    command
        .args([
            "--exact",
            &format!(
                "{}::stdin_target",
                module_path!().split_once("::").unwrap().1
            ),
            "--ignored",
            "--nocapture",
        ])
        .stdin(input)
        .env("APP_ENV", "test")
        .env("RYFRAME_OLD_PROTOCOL", "must-not-leak")
        .env("VITE_OLD_PROTOCOL", "must-not-leak")
        .env("SNOWFLAKE_WORKER_ID", "must-not-leak")
        .env("APP_DATABASE_PASSWORD", "must-not-leak");
    command
}

fn input_file(bytes: &[u8], offset: u64) -> Stdio {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(bytes).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    Stdio::from(file)
}

fn bounded_output(mut command: Command) -> (ExitStatus, Vec<u8>, Vec<u8>) {
    let mut stdout = tempfile::tempfile().unwrap();
    let mut stderr = tempfile::tempfile().unwrap();
    command
        .stdout(stdout.try_clone().unwrap())
        .stderr(stderr.try_clone().unwrap());
    let executable = env::current_exe().unwrap();
    let command = if command.get_program() == executable.as_os_str() {
        trampoline_command(
            command,
            Path::new("/delegated/ryframe-devex-1-2-3"),
            &executable,
        )
        .unwrap()
    } else {
        command
    };
    let mut child = ChildGroup::new().unwrap().spawn(command).unwrap();
    let deadline = Instant::now() + CHILD_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "stdin 目标未按期退出");
        thread::sleep(Duration::from_millis(10));
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    stdout.seek(SeekFrom::Start(0)).unwrap();
    stderr.seek(SeekFrom::Start(0)).unwrap();
    stdout.read_to_end(&mut out).unwrap();
    stderr.read_to_end(&mut err).unwrap();
    (status, out, err)
}

#[test]
fn trampoline_preserves_bound_stdin_bytes_offset_and_output_handles() {
    let payload = b"{\"input\":\"\xe4\xb8\xad\xe6\x96\x87\"}\n\0\xff";
    let mut bytes = b"ignored-prefix".to_vec();
    let offset = bytes.len() as u64;
    bytes.extend_from_slice(payload);
    let (status, stdout, stderr) = bounded_output(invocation(input_file(&bytes, offset)));
    assert!(status.success(), "{}", String::from_utf8_lossy(&stderr));
    assert_eq!(
        stdout
            .windows(payload.len())
            .filter(|part| *part == payload)
            .count(),
        1
    );
    assert_eq!(stderr, b"stdin-target-stderr");
}

#[test]
fn trampoline_empty_null_closed_and_oversize_stdin_finish_without_hanging() {
    let (read_end, write_end) = nix::unistd::pipe().unwrap();
    drop(write_end);
    for (input, code) in [
        (input_file(b"", 0), 2),
        (Stdio::null(), 2),
        (Stdio::from(read_end), 2),
        (input_file(&vec![b'x'; INPUT_LIMIT + 1], 0), 3),
    ] {
        let (status, _, stderr) = bounded_output(invocation(input));
        assert_eq!(
            status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&stderr)
        );
    }
    let (status, _, stderr) = bounded_output(invocation(input_file(&vec![b'x'; INPUT_LIMIT], 0)));
    assert!(status.success(), "{}", String::from_utf8_lossy(&stderr));
}

#[test]
fn trampoline_keeps_ordinary_command_arguments_directory_and_exit_status() {
    let directory = tempfile::tempdir().unwrap();
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "exit 7"])
        .current_dir(directory.path())
        .stdin(Stdio::null());
    let executable = env::current_exe().unwrap();
    let trampoline = trampoline_command(
        command,
        Path::new("/delegated/ryframe-devex-1-2-3"),
        &executable,
    )
    .unwrap();
    assert_eq!(trampoline.get_current_dir(), Some(directory.path()));
    assert_eq!(
        trampoline.get_args().collect::<Vec<_>>(),
        ["/bin/sh", "-c", "exit 7"]
    );
    let arguments = trampoline.get_args().map(OsStr::to_owned);
    let mut target = target_command(false, arguments).unwrap();
    target.stdin(Stdio::null());
    let (status, stdout, stderr) = bounded_output(target);
    assert_eq!(status.code(), Some(7));
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn trampoline_marker_round_trips_mode_and_non_utf8_group() {
    let group = PathBuf::from(OsString::from_vec(
        b"/delegated/ryframe-devex-1-\xff-3".to_vec(),
    ));
    for (mode, in_place) in [(IN_PLACE_MODE, true), (EXTERNAL_MODE, false)] {
        let marker = trampoline_marker(mode, &group).unwrap();
        assert_eq!(
            parse_trampoline_marker(&marker).unwrap(),
            Some((in_place, group.clone()))
        );
    }
    assert!(
        parse_trampoline_marker(OsStr::new("ordinary-program"))
            .unwrap()
            .is_none()
    );
    assert!(parse_trampoline_marker(OsStr::new("__ryframe_devex_cgroup:x:/tmp/a")).is_err());
}

#[test]
fn gate_group_name_requires_exact_run_attempt_and_nonce() {
    assert!(valid_gate_name(
        "ryframe-devex-ci-12-3-0123456789abcdef0123456789abcdef"
    ));
    for name in [
        "ryframe-devex-ci-0-3-0123456789abcdef0123456789abcdef",
        "ryframe-devex-ci-12-03-0123456789abcdef0123456789abcdef",
        "ryframe-devex-ci-12-3-0123456789ABCDEF0123456789ABCDEF",
        "ryframe-devex-ci-12-3-0123",
        "ryframe-devex-ci-12-3-0123456789abcdef0123456789abcdef-extra",
    ] {
        assert!(!valid_gate_name(name), "{name}");
    }
}
