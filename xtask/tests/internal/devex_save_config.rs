use super::*;
use crate::devex::model::{CacheState, DevexCommand, DevexSuite};

fn common() -> Option<BaselineContract> {
    Some(BaselineContract::StableReadinessB0CommonToolchainV1)
}

fn test_root() -> tempfile::TempDir {
    let root = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

#[test]
fn paired_save_cli_requires_the_actual_configuration_arguments() {
    let root = test_root();
    let mut args = [
        "paired",
        "--base-backend",
        "base-backend",
        "--candidate-backend",
        "candidate-backend",
        "--base-frontend",
        "base-frontend",
        "--candidate-frontend",
        "candidate-frontend",
        "--baseline-contract",
        "stable-readiness-b0-common-toolchain-v1",
        "--suite",
        "cargo-dev-save",
        "--variant",
        "config-only",
        "--runs",
        "6",
        "--cache",
        "warm",
    ]
    .map(str::to_owned)
    .to_vec();
    assert!(
        crate::devex::parse_command(&args)
            .unwrap_err()
            .contains("--base-config-dir")
    );
    args.extend([
        "--base-config-dir".into(),
        root.path().join("B0").to_str().unwrap().into(),
        "--candidate-config-dir".into(),
        root.path().join("B1").to_str().unwrap().into(),
    ]);
    let DevexCommand::Paired(options) = crate::devex::parse_command(&args).unwrap() else {
        panic!("需要配对保存测量命令");
    };
    let paths = options.save_config_dirs.unwrap();
    assert_eq!(paths.baseline, root.path().join("B0"));
    assert_eq!(paths.candidate, root.path().join("B1"));
}

#[test]
fn save_config_flags_are_required_only_for_seven_common_save_cases() {
    let root = test_root();
    let baseline = root.path().join("B0");
    let candidate = root.path().join("B1");
    for variant in [
        "config-only",
        "api-only",
        "worker-only",
        "shared-runtime",
        "locales",
        "migration-only",
        "resource-manifest",
        "cancellation",
    ] {
        let run = DevexRunOptions {
            suite: DevexSuite::CargoDevSave,
            variant: variant.into(),
            cache_state: CacheState::Warm,
            runs: 1,
        };
        let allowed = variant != "cancellation";
        assert_eq!(
            SaveConfigDirs::select(&run, common(), None, None).is_err(),
            allowed
        );
        assert!(SaveConfigDirs::select(&run, common(), Some(baseline.clone()), None).is_err());
        let flags = Some(baseline.clone());
        assert_eq!(
            SaveConfigDirs::select(&run, common(), flags.clone(), Some(candidate.clone())).is_ok(),
            allowed
        );
        assert!(SaveConfigDirs::select(&run, None, flags, Some(candidate.clone())).is_err());
        assert!(
            SaveConfigDirs::select(
                &run,
                common(),
                Some("relative".into()),
                Some(candidate.clone())
            )
            .is_err()
        );
    }
}

#[test]
fn save_config_binds_only_metadata_and_verifies_exact_restoration() {
    let root = test_root();
    let baseline = root.path().join("B0");
    let candidate = root.path().join("B1");
    for path in [&baseline, &candidate] {
        fs::create_dir(path).unwrap();
        fs::write(path.join(DEV_APP), b"password = \"fixture-private\"").unwrap();
    }
    let dirs = SaveConfigDirs {
        baseline: baseline.clone(),
        candidate: candidate.clone(),
    };
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    let source = source.canonicalize().unwrap();
    assert!(
        dirs.bind([&source; 4])
            .unwrap_err()
            .to_string()
            .contains(COMMON_APP)
    );
    fs::write(baseline.join(COMMON_APP), b"").unwrap();
    let (baseline, candidate) = dirs.bind([&source; 4]).unwrap();
    assert!(baseline.files.contains_key(COMMON_APP));
    assert!(!candidate.files.contains_key(COMMON_APP));
    let serialized = serde_json::to_string(&candidate).unwrap();
    assert!(!serialized.contains("fixture-private"));
    let bound: SaveConfigBinding = serde_json::from_str(&serialized).unwrap();
    let dev = bound.directory.join(DEV_APP);
    let original = fs::read(&dev).unwrap();
    fs::write(&dev, [original.as_slice(), b"\n# save"].concat()).unwrap();
    assert!(bound.verify().is_err());
    fs::write(dev, original).unwrap();
    bound.verify().unwrap();
    fs::write(bound.directory.join("tools.toml"), b"").unwrap();
    assert!(bound.verify().is_err());
}

#[test]
fn save_config_refuses_overlapping_roots_and_non_file_overlay() {
    let root = test_root();
    let baseline = root.path().join("B0");
    let candidate = baseline.join("B1");
    fs::create_dir_all(&candidate).unwrap();
    let dirs = SaveConfigDirs {
        baseline: baseline.clone(),
        candidate,
    };
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    let source = source.canonicalize().unwrap();
    assert!(
        dirs.bind([&source; 4])
            .unwrap_err()
            .to_string()
            .contains("重叠")
    );
    let candidate = root.path().join("B1");
    fs::create_dir(&candidate).unwrap();
    let dirs = SaveConfigDirs {
        baseline: baseline.clone(),
        candidate,
    };
    assert!(
        dirs.bind([&baseline.canonicalize().unwrap(); 4])
            .unwrap_err()
            .to_string()
            .contains("重叠")
    );
    fs::create_dir(baseline.join(DEV_APP)).unwrap();
    assert!(
        dirs.bind([&source; 4])
            .unwrap_err()
            .to_string()
            .contains("普通文件")
    );
}
