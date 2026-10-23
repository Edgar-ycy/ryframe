//! 不连接数据库的客户端替身，检查私有文件、进程树回收和失败时的输出隔离。
use std::{env, fs, io::{self, Read}, process::{self, Command}, thread, time::Duration};

fn main() {
    if env::args().any(|v| v == "--descendant") {
        thread::sleep(Duration::from_secs(60));
        return;
    }
    let args = env::args().skip(1).collect::<Vec<_>>();
    let options = args[0].strip_prefix("--defaults-file=").unwrap();
    let password = fs::read_to_string(options).unwrap();
    assert!(password.starts_with("[client]\npassword=\""));
    assert!(!args.iter().any(|v| v.contains("private-password")));
    assert!(!env::vars().any(|(key,_)| key.starts_with("APP_") || key == "MYSQL_PWD"));
    let mut sql = String::new();
    io::stdin().read_to_string(&mut sql).unwrap();
    assert_eq!(sql.matches("CREATE DATABASE").count(), 4);
    assert!(!sql.contains("IF NOT EXISTS"));
    fs::write("observed-sql.txt", sql).unwrap();
    fs::write("observed-options.txt", password).unwrap();
    let mode = args.iter().find_map(|v| v.strip_prefix("--user=")).unwrap();
    if mode == "fail" {
        eprintln!("private-password: simulated partial DDL");
        process::exit(17);
    }
    if mode == "crash" {
        process::abort();
    }
    if mode == "hang" || mode == "orphan" {
        let child = Command::new(env::current_exe().unwrap()).arg("--descendant").spawn().unwrap();
        fs::write("descendant.pid", child.id().to_string()).unwrap();
        fs::write("client.pid", process::id().to_string()).unwrap();
        if mode == "orphan" { return; }
        thread::sleep(Duration::from_secs(60));
    }
}
