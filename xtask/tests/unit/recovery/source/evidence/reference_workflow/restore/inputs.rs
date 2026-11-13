use super::*;
use crate::recovery::clone::native::json::write_new;

struct Fixture {
    _directory: tempfile::TempDir,
    backend: PathBuf,
    local: PathBuf,
    backup: PathBuf,
    initialized: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&base).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("restore-inputs-")
            .tempdir_in(base)
            .unwrap();
        let backend = portable_path(&directory.path().canonicalize().unwrap());
        let local = backend.join(".local-tests");
        let backup = local.join("source/backup");
        let initialized = local.join("fresh/initialized.json");
        fs::create_dir_all(&backup).unwrap();
        fs::create_dir_all(initialized.parent().unwrap()).unwrap();
        Self {
            _directory: directory,
            backend,
            local,
            backup,
            initialized,
        }
    }

    fn read(&self, name: &str, content: &Value) -> Document {
        let path = self.local.join(name);
        write_new(&path, content).unwrap();
        document::read(&path, &self.backend, "恢复输入夹具").unwrap()
    }

    fn output(&self, work: &Path, side: &str, target_side: &str) -> Result<PathBuf> {
        output(
            &self.backend,
            &json!({"work_dir":work,"target_side":side}),
            &json!({"target_side":target_side,"fresh_target":{"initialized":{"path":self.initialized}}}),
            &self.backup,
        )
    }
}

#[test]
fn output_preflight_is_read_only_with_missing_work_and_rejects_each_existing_phase() {
    let fixture = Fixture::new();
    let work = fixture.local.join("new-work");
    let receipt = work.join("restore-base.json");
    assert_eq!(fixture.output(&work, "base", "base").unwrap(), receipt);
    assert_eq!(
        fixture.output(&work, "candidate", "candidate").unwrap(),
        work.join("restore-candidate.json")
    );
    assert!(!work.exists());
    assert!(fixture.output(&work, "candidate", "base").is_err());
    fs::create_dir(&work).unwrap();
    for phase in ["before", "after", "failure-after"] {
        let directory = work.join(format!("restore-base-{phase}"));
        fs::create_dir(&directory).unwrap();
        assert!(fixture.output(&work, "base", "base").is_err());
        assert!(directory.is_dir());
        fs::remove_dir(directory).unwrap();
    }
    fs::write(&receipt, b"original").unwrap();
    assert!(fixture.output(&work, "base", "base").is_err());
    assert_eq!(fs::read(&receipt).unwrap(), b"original");
}

#[test]
fn output_cannot_be_created_under_backup_or_initialized_evidence() {
    let fixture = Fixture::new();
    for root in [
        fixture.backup.as_path(),
        fixture.initialized.parent().unwrap(),
    ] {
        let nested = root.join("missing/nested");
        assert!(fixture.output(&nested, "base", "base").is_err());
        assert!(!nested.exists());
    }
}

#[test]
fn backup_root_rejects_a_different_existing_directory_and_manifest_location() {
    let fixture = Fixture::new();
    let result = json!({"backup_root":fixture.backup});
    let manifest = fixture.backup.join("manifest.json");
    assert_eq!(
        backup_root(&fixture.backend, &fixture.backup, &result, &manifest).unwrap(),
        fixture.backup
    );
    let alternative = fixture.local.join("other-backup");
    fs::create_dir(&alternative).unwrap();
    assert!(backup_root(&fixture.backend, &alternative, &result, &manifest).is_err());
    assert!(
        backup_root(
            &fixture.backend,
            &fixture.backup,
            &result,
            &alternative.join("manifest.json")
        )
        .is_err()
    );
}

#[test]
fn registered_file_and_checkpoint_reject_same_content_copies_and_wrong_bindings() {
    let fixture = Fixture::new();
    let target = fixture.read("target.json", &json!({"target_side":"base"}));
    let registration = fixture.read(
        "registration.json",
        &json!({"target_plan":document::binding(&target)}),
    );
    let target_binding = document::binding(&target);
    assert_eq!(
        registered_path(&fixture.backend, &target.path, &target_binding).unwrap(),
        target.path
    );
    let copy = fixture.read("copy.json", &target.value);
    assert_eq!(copy.raw, target.raw);
    assert!(registered_path(&fixture.backend, &copy.path, &target_binding).is_err());
    let checkpoint =
        json!({"registration":document::binding(&registration),"target_plan":target_binding});
    check_runtime(&registration, &target, &checkpoint).unwrap();
    for field in ["registration", "target_plan"] {
        let mut changed = checkpoint.clone();
        changed[field]["sha256"] = json!("0".repeat(64));
        assert!(check_runtime(&registration, &target, &changed).is_err());
    }
    let mut extra = checkpoint;
    extra["running"] = json!(true);
    assert!(check_runtime(&registration, &target, &extra).is_err());
}

#[test]
fn provenance_window_checks_original_bytes_and_protects_both_bound_files() {
    for changed in ["comparison.json", "product.json"] {
        let fixture = Fixture::new();
        let comparison = fixture.read("comparison.json", &json!({"value":"original"}));
        let product = fixture.read("product.json", &json!({"value":"original"}));
        let path = fixture.local.join(changed);
        let original = fs::read(&path).unwrap();
        let changed_bytes = std::str::from_utf8(&original)
            .unwrap()
            .replace("original", "modified");
        assert_eq!(changed_bytes.len(), original.len());
        fs::write(&path, &changed_bytes).unwrap();
        assert!(hold_provenance(&comparison, &product).is_err(), "{changed}");
        fs::write(&path, &original).unwrap();

        let held = hold_provenance(&comparison, &product).unwrap();
        check_provenance(&held).unwrap();
        let alternate = fixture.local.join("alternate.json");
        fs::write(&alternate, &original).unwrap();
        let written = fs::write(&path, &changed_bytes);
        #[cfg(windows)]
        {
            assert_eq!(written.unwrap_err().raw_os_error(), Some(32), "{changed}");
            assert_eq!(fs::remove_file(&path).unwrap_err().raw_os_error(), Some(32));
            assert_eq!(fs::read(&path).unwrap(), original);
            held.check().unwrap();
        }
        #[cfg(not(windows))]
        {
            written.unwrap();
            assert!(check_provenance(&held).is_err(), "{changed}");
            fs::write(&path, &original).unwrap();
            fs::rename(&alternate, &path).unwrap();
            assert!(check_provenance(&held).is_err(), "{changed}");
        }
    }
}
