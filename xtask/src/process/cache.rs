use std::{
    env,
    ffi::OsString,
    process::{Command, Stdio},
    sync::{Once, OnceLock},
    thread,
    time::{Duration, Instant},
};

enum RustcCache {
    ExistingWrapper(OsString),
    Sccache,
    Cargo,
}

pub(crate) fn rustc_cache_label() -> &'static str {
    match RUSTC_CACHE.get_or_init(resolve_rustc_cache) {
        RustcCache::ExistingWrapper(_) => "existing-wrapper",
        RustcCache::Sccache => "sccache",
        RustcCache::Cargo => "cargo",
    }
}

static RUSTC_CACHE: OnceLock<RustcCache> = OnceLock::new();
static RUSTC_CACHE_NOTICE: Once = Once::new();

pub(super) fn configure_cargo_cache(executable: &str, command: &mut Command) {
    if executable != "cargo" {
        return;
    }
    match RUSTC_CACHE.get_or_init(resolve_rustc_cache) {
        RustcCache::ExistingWrapper(wrapper) => {
            RUSTC_CACHE_NOTICE.call_once(|| {
                println!(
                    "检测到 RUSTC_WRAPPER={}，保留现有编译缓存配置。",
                    wrapper.to_string_lossy()
                );
            });
        }
        RustcCache::Sccache => {
            command.env("RUSTC_WRAPPER", "sccache");
            RUSTC_CACHE_NOTICE.call_once(|| println!("使用 sccache 复用 Rust 编译缓存。"));
        }
        RustcCache::Cargo => {
            RUSTC_CACHE_NOTICE.call_once(|| {
                println!("未检测到可正常运行的 sccache，使用 Cargo 本地缓存继续执行。");
            });
        }
    }
}

fn resolve_rustc_cache() -> RustcCache {
    if let Some(wrapper) = env::var_os("RUSTC_WRAPPER").filter(|value| !value.is_empty()) {
        return RustcCache::ExistingWrapper(wrapper);
    }
    if sccache_can_wrap_rustc() {
        RustcCache::Sccache
    } else {
        RustcCache::Cargo
    }
}

/// sccache 能被找到不代表其服务可以启动。用一次短暂的真实编译器调用探测，
/// 避免远端缓存或后台服务故障让开发命令整体失败。
fn sccache_can_wrap_rustc() -> bool {
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let mut child = match Command::new("sccache")
        .arg(rustc)
        .arg("-vV")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}
