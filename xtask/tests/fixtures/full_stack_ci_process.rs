//! 无网络的协调器进程替身。
use std::{
    env, fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};
fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mode = &args[0];
    let directory = Path::new(&args[1]);
    if mode == "child" {
        thread::sleep(Duration::from_secs(60));
        return;
    }
    fs::write(directory.join("parent.pid"), std::process::id().to_string()).unwrap();
    if mode == "hang" || mode == "orphan" {
        let child = Command::new(env::current_exe().unwrap())
            .args(["child", &args[1]])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        fs::write(directory.join("child.pid"), child.id().to_string()).unwrap();
        if mode == "orphan" {
            return;
        }
        thread::sleep(Duration::from_secs(60));
        return;
    }
    if mode == "fail" {
        eprintln!("private-password");
        std::process::exit(17);
    }
    if mode == "compiler-fail" {
        eprintln!("private-password: fatal error C1083: Cannot open compiler generated file: Invalid argument");
        std::process::exit(17);
    }
    if mode == "json" {
        eprintln!("编译诊断 private-password");
        println!("{{\"reason\":\"compiler-artifact\"}}");
        return;
    }
    if mode == "flood" {
        let _ = std::io::stdout().write_all(&vec![b'x'; 17 * 1024 * 1024]);
        return;
    }
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    std::io::stdout().write_all(&input).unwrap();
}
