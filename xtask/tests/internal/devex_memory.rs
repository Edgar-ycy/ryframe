use std::{
    fs,
    process::{Command, Stdio},
    time::Duration,
};

use serde_json::{Value, json};

use super::devex::{
    distribution,
    memory::{self, MemoryEvidence, MemoryMethod, MemoryReading},
};

pub(super) fn evidence() -> Value {
    let mut evidence = MemoryEvidence::new();
    evidence.steps = vec![MemoryReading::Measured { peak_bytes: 1024 }];
    serde_json::to_value(evidence).unwrap()
}

#[test]
fn p99_uses_nearest_rank_without_changing_p95() {
    let values = (1..=100).map(f64::from).collect::<Vec<_>>();
    let result = distribution(&values).unwrap();
    assert_eq!((result.p50, result.p95, result.p99), (50.0, 95.0, 99.0));
    assert!(distribution(&[]).is_none());
}

#[test]
fn memory_rejects_invalid_readings_and_tampered_collector_identity() {
    let mut record: MemoryEvidence = serde_json::from_value(evidence()).unwrap();
    assert!(record.validate().is_ok());
    for reading in [
        MemoryReading::Measured { peak_bytes: 0 },
        MemoryReading::unavailable(" "),
    ] {
        record.steps = vec![reading];
        assert!(record.validate().is_err());
    }
    record.steps = vec![MemoryReading::Measured { peak_bytes: 10 }];
    record.method = MemoryMethod::Unsupported;
    assert!(record.validate().is_err());
    assert!(matches!(
        MemoryReading::from_result(Ok(0)),
        MemoryReading::Unavailable { .. }
    ));

    let mut tampered = evidence();
    tampered["collector"]["version"] = json!(2);
    let tampered: MemoryEvidence = serde_json::from_value(tampered).unwrap();
    assert!(
        tampered
            .validate()
            .unwrap_err()
            .to_string()
            .contains("语义指纹")
    );
}

#[test]
fn memory_summary_preserves_failed_samples_and_blocks_mixed_or_missing_collectors() {
    let run = super::devex_tests::fake_run("memory-failure", "sha256:same", &[100.0, 200.0]);
    let path = run.join("samples.jsonl");
    let mut samples = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    samples[1]["status"] = json!("failed");
    samples[1]["exit_code"] = json!(1);
    samples[1]["memory"]["steps"][0]["peak_bytes"] = json!(4096);
    save(&path, &samples);
    let summary = super::devex::summarize(&run).unwrap();
    let stats = summary.memory.unwrap();
    assert_eq!(stats.successful_peak_bytes.as_ref().unwrap().max, 1024.0);
    assert_eq!(stats.failed_peak_bytes.as_ref().unwrap().max, 4096.0);
    assert_eq!(summary.duration_ms.unwrap().p99, 100.0);
    let report = fs::read_to_string(run.join("summary.md")).unwrap();
    assert!(report.contains("失败样本：峰值 4096 bytes"));
    assert!(report.contains("- 采集器："));

    let mut mixed = stats.clone();
    mixed.method = match stats.method {
        MemoryMethod::LinuxCgroupCharge => MemoryMethod::WindowsJobCommit,
        _ => MemoryMethod::LinuxCgroupCharge,
    };
    assert!(memory::ensure_comparable(Some(&stats), Some(&mixed)).is_err());
    samples[1]["memory"]["steps"] = json!([{"status":"unavailable","reason":"采集失败"}]);
    save(&path, &samples);
    let missing = super::devex::summarize(&run).unwrap().memory.unwrap();
    assert_eq!(missing.unavailable_samples, 1);
    assert!(memory::ensure_comparable(Some(&stats), Some(&missing)).is_err());
    samples[1]["memory"]["collector"]["fingerprint"] = json!(format!("sha256:{}", "0".repeat(64)));
    save(&path, &samples);
    assert!(super::devex::summarize(&run).is_err());
    fs::remove_dir_all(run).unwrap();
}

fn save(path: &std::path::Path, samples: &[Value]) {
    fs::write(
        path,
        samples
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
}

fn child_command(role: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "devex_memory_tests::memory_child", "--nocapture"])
        .env("RYFRAME_DEVEX_MEMORY_CHILD", role)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command
}

#[test]
fn memory_child() {
    let Ok(role) = std::env::var("RYFRAME_DEVEX_MEMORY_CHILD") else {
        return;
    };
    assert!(
        std::env::var_os("RYFRAME_DEVEX_CGROUP_TRAMPOLINE").is_none(),
        "私有跳板标记不得泄漏给目标程序"
    );
    #[cfg(target_os = "linux")]
    {
        let membership = fs::read_to_string("/proc/self/cgroup").unwrap();
        assert!(
            membership.contains("/ryframe-devex-"),
            "目标逻辑开始前尚未进入本次隔离 cgroup：{membership}"
        );
    }
    let mut allocation = vec![0_u8; 32 * 1024 * 1024];
    for page in allocation.chunks_mut(4096) {
        page[0] = 1;
    }
    if role == "parent" || role == "failed-parent" {
        assert!(child_command("grandchild").status().unwrap().success());
    } else if role == "orphan-parent" {
        let mut child = child_command("orphan").spawn().unwrap();
        // 主线程先退出以制造范围不完整；外层 ManagedChild 拥有并回收整棵树。
        std::thread::spawn(move || child.wait().unwrap());
    } else {
        std::thread::sleep(Duration::from_millis(if role == "orphan" {
            5000
        } else {
            100
        }));
    }
    std::hint::black_box(&allocation);
    if role == "failed-parent" {
        std::process::exit(3);
    }
}

#[cfg(windows)]
#[test]
fn windows_job_covers_grandchildren_and_preserves_failed_command_memory() {
    for role in ["parent", "failed-parent"] {
        let (status, reading) = memory::execute(child_command(role)).unwrap();
        assert_eq!(status.success(), role == "parent");
        let MemoryReading::Measured { peak_bytes } = reading else {
            panic!("{reading:?}");
        };
        assert!(
            peak_bytes >= 64 * 1024 * 1024,
            "没有计入同时存活的孙进程：{peak_bytes}"
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_job_marks_descendants_outliving_the_command_as_incomplete() {
    let (status, reading) = memory::execute(child_command("orphan-parent")).unwrap();
    assert!(status.success());
    assert!(matches!(reading, MemoryReading::Unavailable { .. }));
}

#[cfg(target_os = "linux")]
#[test]
fn linux_owned_group_name_requires_exact_numeric_identity() {
    use std::ffi::OsStr;

    assert!(memory::linux::valid_group_name(Some(OsStr::new(
        "ryframe-devex-123-456-0"
    ))));
    for value in [
        "ryframe-devex-123-456",
        "ryframe-devex-123-456-0-extra",
        "ryframe-devex-123-a-0",
        "foreign-123-456-0",
    ] {
        assert!(
            !memory::linux::valid_group_name(Some(OsStr::new(value))),
            "{value}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "需要显式提供具有 memory controller 委托权限的 RYFRAME_DEVEX_CGROUP_ROOT"]
fn linux_cgroup_joins_before_exec_covers_grandchildren_and_cleans_its_group() {
    let root = std::env::var_os("RYFRAME_DEVEX_CGROUP_ROOT").expect("缺少显式 cgroup 委托目录");
    let entries = || {
        fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<std::collections::BTreeSet<_>>()
    };
    let before = entries();
    for role in ["parent", "failed-parent"] {
        let (status, reading) = memory::linux::execute_with_trampoline(
            child_command(role),
            std::path::PathBuf::from(env!("CARGO_BIN_EXE_xtask")),
        )
        .unwrap();
        assert_eq!(status.success(), role == "parent");
        let MemoryReading::Measured { peak_bytes } = reading else {
            panic!("{reading:?}");
        };
        assert!(peak_bytes >= 64 * 1024 * 1024);
        assert_eq!(entries(), before);
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "需要显式提供具有 memory controller 委托权限的 RYFRAME_DEVEX_CGROUP_ROOT"]
fn linux_cgroup_fails_closed_and_reaps_descendants_that_outlive_command() {
    let root = std::env::var_os("RYFRAME_DEVEX_CGROUP_ROOT").expect("缺少显式 cgroup 委托目录");
    let before = fs::read_dir(&root).unwrap().count();
    let (status, reading) = memory::linux::execute_with_trampoline(
        child_command("orphan-parent"),
        std::path::PathBuf::from(env!("CARGO_BIN_EXE_xtask")),
    )
    .unwrap();
    assert!(status.success());
    assert!(matches!(reading, MemoryReading::Unavailable { .. }));
    assert_eq!(fs::read_dir(root).unwrap().count(), before);
}
