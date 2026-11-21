//! 隔离 API/Worker 替身：按配置角色选择监听值，不增加测试环境变量。
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
    if matches!(args[0].as_str(), "descendant" | "descendant-tree") {
        let marker = Path::new(&args[1]);
        if args[0] == "descendant-tree" {
            spawn_descendant("descendant", &marker.with_file_name("grandchild.pid"));
        }
        fs::write(marker, std::process::id().to_string()).unwrap();
        thread::sleep(Duration::from_secs(120));
        return;
    }
    assert_eq!(args[0], "--config-dir");
    assert_eq!(env::var("APP_ENV").unwrap(), "test");
    assert!(!env::vars_os().any(|(key, _)| {
        let key = key.to_string_lossy().to_ascii_uppercase();
        (key.starts_with("APP_") && key != "APP_ENV")
            || key.starts_with("RYFRAME_")
            || key.starts_with("AWS_")
            || key.starts_with("VITE_")
            || key.starts_with("SNOWFLAKE_")
            || key == "MYSQL_PWD"
    }));
    let config = Path::new(&args[1]);
    let root = config.parent().unwrap();
    let (started_marker, host, port) = listener(config);
    let role_mode = root.join(if started_marker == "worker-started" {
        "worker-mode"
    } else {
        "api-mode"
    });
    let mode = if role_mode.exists() {
        fs::read_to_string(role_mode).unwrap()
    } else {
        fs::read_to_string(root.join("mode")).unwrap()
    };
    fs::write(root.join(started_marker), std::process::id().to_string()).unwrap();
    if mode.starts_with("tree") {
        let deep = mode == "tree-deep";
        let descendant = root.join("descendant.pid");
        let grandchild = root.join("grandchild.pid");
        let action = if deep {
            "descendant-tree"
        } else {
            "descendant"
        };
        spawn_descendant(action, &descendant);
        while !descendant.exists() || (deep && !grandchild.exists()) {
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
    let listener = TcpListener::bind((host.as_str(), port)).unwrap();
    listener.set_nonblocking(true).unwrap();
    loop {
        if mode == "tree-ready-json" && root.join("api-natural-exit").is_file() {
            return;
        }
        if root.join("disable-ready").exists() {
            drop(listener);
            thread::sleep(Duration::from_secs(120));
            return;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let mut bytes = [0; 4096];
                let _ = stream.read(&mut bytes);
                if mode == "tree-ready-json" {
                    let body =
                        br#"{"status":"ready","mysql":"up","redis":"up","object_storage":"up"}"#;
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    ).and_then(|()| stream.write_all(body));
                } else {
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(error) => panic!("{error}"),
        }
    }
}

fn spawn_descendant(action: &str, marker: &Path) {
    let mut child = Command::new(env::current_exe().unwrap());
    child
        .arg(action)
        .arg(marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        child.creation_flags(0x0800_0000);
    }
    child.spawn().unwrap();
}

fn listener(config: &Path) -> (&'static str, String, u16) {
    let (started_marker, section, host, port) =
        match config.file_name().and_then(|name| name.to_str()) {
            Some("config") => ("fixture-api-started", "[app]", "host=", "port="),
            Some("worker-config") => ("worker-started", "[jobs]", "health_host=", "health_port="),
            _ => panic!("替身只能使用明确的 API 或 Worker 配置目录"),
        };
    let text = fs::read_to_string(config.join("app.test.toml")).unwrap();
    let lines = text
        .lines()
        .map(str::trim)
        .skip_while(|line| *line != section)
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .collect::<Vec<_>>();
    let host = lines
        .iter()
        .find_map(|line| line.strip_prefix(host))
        .unwrap()
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap()
        .to_owned();
    let port = lines
        .iter()
        .find_map(|line| line.strip_prefix(port))
        .unwrap()
        .parse()
        .unwrap();
    (started_marker, host, port)
}
