//! 隔离 Worker 替身：所有控制值来自文件，不增加测试环境变量。
use std::{
    env, fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};
fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args[0] == "descendant" {
        fs::write(&args[1], std::process::id().to_string()).unwrap();
        thread::sleep(Duration::from_secs(120));
        return;
    }
    assert_eq!(args[0], "--config-dir");
    assert_eq!(env::var("APP_ENV").unwrap(), "test");
    assert!(
        !env::vars().any(|(key, _)| key.starts_with("APP_") && key != "APP_ENV"
            || key.starts_with("RYFRAME_")
            || key.starts_with("AWS_")
            || key.starts_with("VITE_")
            || key.starts_with("SNOWFLAKE_"))
    );
    let config = Path::new(&args[1]);
    let root = config.parent().unwrap();
    let text = fs::read_to_string(config.join("app.test.toml")).unwrap();
    let port = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("health_port="))
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let mode = fs::read_to_string(root.join("mode")).unwrap();
    fs::write(root.join("worker-started"), std::process::id().to_string()).unwrap();
    if mode.starts_with("tree") {
        let mut child = Command::new(env::current_exe().unwrap());
        child
            .arg("descendant")
            .arg(root.join("descendant.pid"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            child.creation_flags(0x0800_0000);
        }
        child.spawn().unwrap();
        while !root.join("descendant.pid").exists() {
            thread::sleep(Duration::from_millis(10));
        }
    }
    if matches!(mode.as_str(), "exit" | "tree-exit") {
        std::process::exit(7);
    }
    if mode == "idle" {
        thread::sleep(Duration::from_secs(120));
        return;
    }
    if mode == "delay" {
        thread::sleep(Duration::from_secs(1));
    }
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    listener.set_nonblocking(true).unwrap();
    loop {
        if root.join("disable-ready").exists() {
            drop(listener);
            thread::sleep(Duration::from_secs(120));
            return;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let mut bytes = [0; 4096];
                let _ = stream.read(&mut bytes);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(error) => panic!("{error}"),
        }
    }
}
