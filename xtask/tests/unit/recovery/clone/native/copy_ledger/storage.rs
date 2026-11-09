use std::{cell::Cell, fs};

use super::*;
use crate::recovery::clone::native::lifecycle::lock::ProcessGuard;

const FIRST_SHA: &str = "0c1b90dcfe7e62b344449706eb37c61a64bed537159425cc0d644fb59eb8a219";
const NEXT_SHA: &str = "1a28e6aed184403d3192bb18b068223d0b77f01f3f389b61cdc1c88f8d5a19c1";

struct Fixture {
    guard: ProcessGuard,
    _directory: tempfile::TempDir,
    backend: PathBuf,
    root: PathBuf,
    plan: String,
    generation: String,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let backend = crate::recovery::fixture_control::portable_path(
            &directory.path().canonicalize().unwrap(),
        );
        let root = backend.join(".local-tests").join("copy-journal");
        fs::create_dir_all(&root).unwrap();
        let guard = ProcessGuard::acquire_named(&root, GUARD).unwrap();
        fs::create_dir(root.join(LOCK)).unwrap();
        Self {
            guard,
            _directory: directory,
            backend,
            root,
            plan: "a".repeat(64),
            generation: "b".repeat(64),
        }
    }

    fn owner(&self, root: &Path) -> Result<()> {
        if root != self.root {
            return Err("测试锁不属于本账本".into());
        }
        self.guard.check()
    }

    fn open<'a>(&'a self, owner: &'a dyn Fn(&Path) -> Result<()>) -> Result<Storage<'a>> {
        Storage::open(
            &self.backend,
            &self.root,
            &self.plan,
            &self.generation,
            owner,
        )
    }

    fn create<'a>(&'a self, owner: &'a dyn Fn(&Path) -> Result<()>) -> Result<Storage<'a>> {
        Storage::create(
            &self.backend,
            &self.root,
            &self.plan,
            &self.generation,
            owner,
        )
    }

    fn first_line(&self) -> String {
        format!(
            "{{\"event\":{{\"generation_sha256\":\"{}\",\"plan_sha256\":\"{}\",\"type\":\"created\"}},\"previous\":\"{}\",\"sequence\":0,\"sha256\":\"{FIRST_SHA}\"}}\n",
            self.generation,
            self.plan,
            "0".repeat(64),
        )
    }

    fn next_line(&self) -> String {
        format!(
            "{{\"event\":{{\"mode\":\"apply\",\"session\":\"session-example\",\"type\":\"opened\"}},\"previous\":\"{FIRST_SHA}\",\"sequence\":1,\"sha256\":\"{NEXT_SHA}\"}}\n",
        )
    }

    fn head_bytes(&self, sequence: usize, sha: &str) -> String {
        format!(
            "{{\"format_version\":1,\"generation_sha256\":\"{}\",\"plan_sha256\":\"{}\",\"sequence\":{sequence},\"sha256\":\"{sha}\"}}\n",
            self.generation, self.plan,
        )
    }

    fn install_python_bytes(&self) {
        fs::write(self.root.join(JOURNAL), self.first_line()).unwrap();
        fs::write(self.root.join(HEAD), self.head_bytes(0, FIRST_SHA)).unwrap();
    }

    fn raw(&self, name: &str) -> Vec<u8> {
        fs::read(self.root.join(name)).unwrap()
    }
}

fn opened() -> Value {
    json!({"type":"opened","session":"session-example","mode":"apply"})
}

#[test]
fn reads_python_wire_bytes_and_appends_the_exact_next_frame_and_head() {
    let fixture = Fixture::new();
    fixture.install_python_bytes();
    let owner = |root: &Path| fixture.owner(root);
    let mut storage = fixture.open(&owner).unwrap();
    storage.append(opened()).unwrap();
    assert_eq!(
        fixture.raw(JOURNAL),
        (fixture.first_line() + &fixture.next_line()).as_bytes(),
    );
    assert_eq!(
        fixture.raw(HEAD),
        fixture.head_bytes(1, NEXT_SHA).as_bytes()
    );
    assert!(fixture.open(&owner).is_ok());
}

#[test]
fn create_is_exclusive_and_repeated_created_poisoning_preserves_the_first_frame() {
    let fixture = Fixture::new();
    let owner = |root: &Path| fixture.owner(root);
    let mut storage = fixture.create(&owner).unwrap();
    assert_eq!(fixture.raw(JOURNAL), fixture.first_line().as_bytes());
    assert_eq!(
        fixture.raw(HEAD),
        fixture.head_bytes(0, FIRST_SHA).as_bytes()
    );
    assert!(fixture.create(&owner).is_err());
    assert!(
        storage
            .append(created(&fixture.plan, &fixture.generation))
            .is_err()
    );
    assert!(storage.append(opened()).is_err());
    assert_eq!(fixture.raw(JOURNAL), fixture.first_line().as_bytes());
    assert_eq!(
        fixture.raw(HEAD),
        fixture.head_bytes(0, FIRST_SHA).as_bytes()
    );

    for (existing, missing) in [(HEAD, JOURNAL), (JOURNAL, HEAD)] {
        let fixture = Fixture::new();
        let owner = |root: &Path| fixture.owner(root);
        fs::write(fixture.root.join(existing), b"already there").unwrap();
        assert!(fixture.create(&owner).is_err());
        assert_eq!(fixture.raw(existing), b"already there");
        assert!(!fixture.root.join(missing).exists());
    }
}

#[test]
fn refuses_truncated_journal_and_changed_head_without_repair_or_retry() {
    for name in [JOURNAL, HEAD] {
        let fixture = Fixture::new();
        fixture.install_python_bytes();
        let owner = |root: &Path| fixture.owner(root);
        let mut storage = fixture.open(&owner).unwrap();
        let original = fixture.raw(name);
        let truncated = &original[..original.len() - 1];
        fs::write(fixture.root.join(name), truncated).unwrap();
        assert!(fixture.open(&owner).is_err());
        assert!(storage.append(opened()).is_err());
        assert_eq!(fixture.raw(name), truncated);
        fs::write(fixture.root.join(name), original).unwrap();
        assert!(storage.append(opened()).is_err());
        assert_eq!(fixture.raw(JOURNAL), fixture.first_line().as_bytes());
        assert_eq!(
            fixture.raw(HEAD),
            fixture.head_bytes(0, FIRST_SHA).as_bytes()
        );
    }
}

#[test]
fn refuses_same_bytes_from_a_replaced_head_or_a_recreated_clone_lock() {
    for name in [HEAD, LOCK] {
        let fixture = Fixture::new();
        fixture.install_python_bytes();
        let owner = |root: &Path| fixture.owner(root);
        let mut storage = fixture.open(&owner).unwrap();
        let moved = fixture.root.join("preserved");
        fs::rename(fixture.root.join(name), &moved).unwrap();
        if name == HEAD {
            fs::copy(&moved, fixture.root.join(name)).unwrap();
        } else {
            fs::create_dir(fixture.root.join(name)).unwrap();
        }
        assert!(storage.append(opened()).is_err());
        assert_eq!(fixture.raw(JOURNAL), fixture.first_line().as_bytes());
        assert_eq!(
            fixture.raw(HEAD),
            fixture.head_bytes(0, FIRST_SHA).as_bytes()
        );
        assert!(moved.exists());
    }
}

#[test]
fn failed_head_publication_preserves_the_appended_journal_old_head_and_pending_bytes() {
    let fixture = Fixture::new();
    fixture.install_python_bytes();
    let reject_after_append = Cell::new(false);
    let initial = fixture.first_line();
    let owner = |root: &Path| -> Result<()> {
        fixture.owner(root)?;
        if reject_after_append.get()
            && fs::metadata(root.join(JOURNAL))?.len() > initial.len() as u64
        {
            return Err("测试停止发布 head".into());
        }
        Ok(())
    };
    let mut storage = fixture.open(&owner).unwrap();
    reject_after_append.set(true);
    assert!(storage.append(opened()).is_err());
    assert_eq!(
        fixture.raw(JOURNAL),
        (initial.clone() + &fixture.next_line()).as_bytes()
    );
    assert_eq!(
        fixture.raw(HEAD),
        fixture.head_bytes(0, FIRST_SHA).as_bytes()
    );
    let pending: Vec<_> = fs::read_dir(&fixture.root)
        .unwrap()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("head.json.") && name.ends_with(".tmp")
        })
        .collect();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        fs::read(pending[0].path()).unwrap(),
        fixture.head_bytes(1, NEXT_SHA).as_bytes()
    );
    reject_after_append.set(false);
    assert!(storage.append(opened()).is_err());
    assert!(fixture.open(&owner).is_err());
}
