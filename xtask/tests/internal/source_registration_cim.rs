use crate::recovery::clone::native::seed::source_registration::quiet::observation::{
    CimObservation, cim,
};
use std::path::Path;

#[test]
fn native_cim_child_observes_current_pid_with_explicit_microsecond_precision() {
    let backend = crate::workspace::root_dir();
    let parent = backend.join(".local-tests");
    std::fs::create_dir_all(&parent).unwrap();
    let run = tempfile::Builder::new()
        .prefix("native-cim-query-")
        .tempdir_in(parent)
        .unwrap();
    let pid = std::process::id();
    let kernel = crate::recovery::clone::native::lifecycle::identity::process(pid)
        .unwrap()
        .unwrap();
    let sample = cim(
        &backend,
        run.path(),
        pid,
        Path::new(env!("CARGO_BIN_EXE_xtask")),
    )
    .unwrap();
    let CimObservation::Observed(creation) = sample else {
        panic!("本进程 CIM 查询未成功：{sample:?}");
    };
    let expected = kernel["started"].as_str().unwrap().parse::<u64>().unwrap();
    assert_eq!(creation.pid, pid);
    assert_eq!(creation.precision_ticks, 10);
    assert!(creation.started.abs_diff(expected) <= 10);
}
