use std::path::Path;

use super::devex::{CacheState, DevexSuite, sample_target};

#[test]
fn compiler_targets_stay_short_for_deep_worktrees() {
    let run = Path::new(
        "D:/project/rustProject/ryframe/ryframe/.local-tests/resource-gate-benchmark/catalog-surface-v4/backend/.local-tests/devex/2026-08-30/a-very-long-resource-gate-run-identifier",
    );
    let target = sample_target(run, DevexSuite::ResourceGate, CacheState::Warm, 1);
    assert!(target.starts_with(
        "D:/project/rustProject/ryframe/ryframe/.local-tests/resource-gate-benchmark/catalog-surface-v4/backend/.local-tests/d"
    ));
    assert!(
        !target
            .to_string_lossy()
            .contains("a-very-long-resource-gate-run-identifier")
    );
}
