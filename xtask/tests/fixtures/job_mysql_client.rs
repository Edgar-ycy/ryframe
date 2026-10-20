//! 用于会话生命周期测试的原生客户端夹具，不连接数据库。

use std::{
    io::{self, BufRead, Write},
    time::Duration,
};

fn main() {
    let executable = std::env::current_exe().expect("夹具路径");
    let mode = executable
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if mode.ends_with("block") {
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    if mode.ends_with("query-error") {
        eprintln!("fixture-private-credential-diagnostic");
        std::process::exit(7);
    }
    if mode.ends_with("warning") {
        eprintln!("Warning: fixture private diagnostic");
    }
    for line in io::stdin().lock().lines() {
        let line = line.expect("夹具输入");
        if line.starts_with("SELECT 'gate-") {
            let sentinel = line
                .strip_prefix("SELECT '")
                .and_then(|value| value.strip_suffix("';"))
                .expect("查询结束标记");
            println!("{sentinel}");
            io::stdout().flush().expect("夹具输出");
        } else if line.contains("ryframe_resource_ownership") {
            println!(
                "{{\"scope\":\"devex-test-scope\",\"marker\":\"ryframe-owner:v1:devex-test-scope:control\",\"server\":\"12345678-1234-1234-1234-123456789abc\"}}"
            );
        } else if line.contains("sys_background_job_attempt") {
            println!(
                "{{\"job_id\":\"201\",\"sequence\":1,\"outcome\":\"succeeded\",\"available_at\":\"2026-09-04T00:00:00Z\",\"started_at\":\"2026-09-04T00:00:01Z\",\"finished_at\":\"2026-09-04T00:00:03Z\",\"closed_at\":\"2026-09-04T00:00:03Z\"}}"
            );
        } else if line.contains("sys_background_job j") {
            println!(
                "{{\"business_id\":\"101\",\"business_tenant\":\"tenant-a\",\"business_status\":\"succeeded\",\"job_id\":\"201\",\"job_tenant\":\"tenant-a\",\"job_type\":\"system.export.execute\",\"status\":\"succeeded\",\"claim_sequence\":1}}"
            );
        } else if line == "ROLLBACK;" {
            std::fs::write(executable.with_extension("rollback"), b"observed").expect("回滚观察");
        } else if line.starts_with("SELECT fixture") {
            respond(&line);
        }
    }
    if mode.ends_with("close-hang") {
        std::thread::sleep(Duration::from_secs(60));
    }
}

fn respond(line: &str) {
    if line.contains("fixture_timeout") {
        std::thread::sleep(Duration::from_secs(60));
    } else if line.contains("fixture_exit") {
        std::process::exit(7);
    } else if line.contains("fixture_utf8") {
        io::stdout()
            .write_all(&[0xff, b'\n'])
            .expect("无效UTF8夹具");
    } else if line.contains("fixture_limit") {
        let block = vec![b'x'; 4 * 1024 * 1024 + 1];
        io::stdout().write_all(&block).expect("超限夹具");
    } else if line.contains("fixture_duplicate") {
        println!("{{\"x\":1,\"x\":2}}");
    } else {
        if line.contains("fixture_stderr") {
            io::stderr()
                .write_all(&vec![b'x'; 8 * 1024 * 1024])
                .expect("stderr夹具");
        }
        let password = std::env::var("MYSQL_PWD").as_deref() == Ok("fixture-only");
        let private_removed = std::env::var_os("FIXTURE_PASSWORD").is_none();
        println!(
            "{{\"pid\":{},\"password\":{},\"private_removed\":{}}}",
            std::process::id(),
            password,
            private_removed
        );
    }
    io::stdout().flush().expect("夹具输出刷新");
}
