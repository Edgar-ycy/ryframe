//! 监控预检协议夹具，仅观察隔离进程环境和 stdin，不连接外部服务。
use std::{
    io::{self, Read, Write},
    time::Duration,
};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    assert!(
        !std::env::vars_os().any(|(name, _)| {
            let name = name.to_string_lossy().to_ascii_uppercase();
            name.starts_with("APP_")
                || name.starts_with("RYFRAME_")
                || name.starts_with("PYTHON")
                || name == "MYSQL_PWD"
        }),
        "项目、凭据及解释器环境不得进入预检进程"
    );
    assert!(!std::env::args().any(|value| value.contains("stdin-only-scope")));
    match mode.as_str() {
        "timeout" => std::thread::sleep(Duration::from_secs(60)),
        "failure" => {
            eprintln!("private-preflight-secret");
            std::process::exit(7);
        }
        "limit" => {
            let _ = io::stdout().write_all(&vec![b'x'; 1024 * 1024 + 2]);
        }
        _ => {
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input).expect("读取私有协议");
            io::stdout().write_all(&input).expect("回传协议夹具");
        }
    }
}
