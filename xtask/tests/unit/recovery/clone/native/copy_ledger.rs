use super::*;

struct Fixture {
    _directory: tempfile::TempDir,
    backend: PathBuf,
    root: PathBuf,
    plan: String,
    generation: String,
    events: Vec<Value>,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("ledger-read-")
            .tempdir_in(backend.join(".local-tests"))
            .unwrap();
        let root = directory.path().to_owned();
        let plan = "a".repeat(64);
        let generation = "b".repeat(64);
        let before = json!({"kind":"object","resource_sha256":"c".repeat(64),
            "exists":false,"bytes":null,"sha256":null,"metadata_sha256":null});
        let after = json!({"kind":"object","resource_sha256":"c".repeat(64),
            "exists":true,"bytes":1,"sha256":"d".repeat(64),"metadata_sha256":"e".repeat(64)});
        let events = vec![
            json!({"type":"created","plan_sha256":plan,"generation_sha256":generation}),
            json!({"type":"opened","session":"session-example","mode":"apply"}),
            json!({"type":"intent","step_id":"step-001","before":before,"after":after,"artifact_sha256":"f".repeat(64)}),
            json!({"type":"confirmed","step_id":"step-001","observed":after}),
            json!({"type":"closed","outcome":"publishing","completed_at":"2026-10-24T00:00:00+00:00"}),
        ];
        let result = Self {
            _directory: directory,
            backend,
            root,
            plan,
            generation,
            events,
        };
        result.publish(true);
        result
    }

    fn publish(&self, complete: bool) {
        let mut previous = "0".repeat(64);
        let mut raw = Vec::new();
        for (sequence, event) in self.events.iter().enumerate() {
            let body = json!({"sequence":sequence,"previous":previous,"event":event});
            previous = digest::plan_hash(&body).unwrap();
            let mut frame = body;
            frame["sha256"] = json!(previous);
            raw.extend(serde_json::to_vec(&frame).unwrap());
            raw.push(b'\n');
        }
        fs::write(self.root.join("events.ndjson"), raw).unwrap();
        self.write(
            "head.json",
            &json!({"format_version":1,"plan_sha256":self.plan,
            "generation_sha256":self.generation,"sequence":self.events.len()-1,"sha256":previous}),
        );
        if complete {
            self.write("complete-session-example.json",&json!({"format_version":1,
                "status":"ledger_evidence_complete","plan_sha256":self.plan,"generation_sha256":self.generation,
                "session":"session-example","journal_sha256":previous,"steps":1,
                "completed_at":"2026-10-24T00:00:00+00:00","target_ready":false,
                "restore_success":false,"runtime_performance_passed":false}));
        }
    }

    fn write(&self, name: &str, value: &Value) {
        let mut raw = serde_json::to_vec(value).unwrap();
        raw.push(b'\n');
        fs::write(self.root.join(name), raw).unwrap();
    }

    fn read(&self) -> Result<Value> {
        inspect(&self.backend, &self.root, &self.plan, &self.generation)
    }

    fn assert_ineligible_completion(&self, receipt: &str) {
        let paths = ["events.ndjson", "head.json", receipt].map(|name| self.root.join(name));
        let [journal, head, receipt] = paths.each_ref().map(|path| fs::read(path).unwrap());
        let verified = verify_journal(&journal, &head, &self.plan, &self.generation).unwrap();
        assert!(require_complete(&verified.projection).is_err());
        assert!(self.read().is_err());
        assert_eq!(
            paths.map(|path| fs::read(path).unwrap()),
            [journal, head, receipt]
        );
    }
}

#[test]
fn completed_ledger_preserves_original_completion_and_never_rewrites_evidence() {
    let fixture = Fixture::new();
    let path = fixture.root.join("complete-session-example.json");
    let before = fs::read(&path).unwrap();
    let actual = fixture.read().unwrap();
    assert_eq!(actual["status"], "ledger_evidence_complete");
    assert_eq!(actual["completed_at"], "2026-10-24T00:00:00+00:00");
    assert_eq!(actual["steps"]["step-001"]["phase"], "confirmed");
    assert_eq!(fixture.read().unwrap(), actual);
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn truncated_noncanonical_and_wrong_completion_remain_untouched_and_fail_closed() {
    let fixture = Fixture::new();
    for name in [
        "events.ndjson",
        "head.json",
        "complete-session-example.json",
    ] {
        let path = fixture.root.join(name);
        let original = fs::read(&path).unwrap();
        let mut truncated = original.clone();
        truncated.pop();
        fs::write(&path, &truncated).unwrap();
        assert!(fixture.read().is_err(), "{name}");
        assert_eq!(fs::read(&path).unwrap(), truncated);
        fs::write(&path, &original).unwrap();
        let mut spaced = vec![b' '];
        spaced.extend(&original);
        fs::write(&path, &spaced).unwrap();
        assert!(fixture.read().is_err(), "{name}");
        fs::write(&path, &original).unwrap();
    }
    assert!(
        inspect(
            &fixture.backend,
            &fixture.root,
            &fixture.plan,
            &"9".repeat(64)
        )
        .is_err()
    );
    let path = fixture.root.join("complete-session-example.json");
    let mut receipt = document::read(&path).unwrap();
    receipt["steps"] = json!(2);
    fixture.write("complete-session-example.json", &receipt);
    assert!(fixture.read().is_err());
    assert_eq!(document::read(&path).unwrap(), receipt);
}

#[test]
fn lock_and_missing_completion_never_turn_into_success_or_automatic_repair() {
    let fixture = Fixture::new();
    let lock = fixture.root.join("clone.lock");
    fs::write(&lock, b"owned lock fixture").unwrap();
    assert_eq!(fixture.read().unwrap()["status"], "needs_reconciliation");
    assert_eq!(fs::read(&lock).unwrap(), b"owned lock fixture");
    fs::remove_file(lock).unwrap();
    let receipt = fixture.root.join("complete-session-example.json");
    fs::remove_file(&receipt).unwrap();
    assert_eq!(fixture.read().unwrap()["status"], "needs_reconciliation");
    assert!(!receipt.exists());
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 2);
}

#[test]
fn forged_completion_cannot_skip_the_original_finish_eligibility_checks() {
    let mut fixture = Fixture::new();
    fixture.events.remove(3);
    fixture.events.remove(2);
    fixture.publish(true);
    let path = fixture.root.join("complete-session-example.json");
    let mut receipt = document::read(&path).unwrap();
    receipt["steps"] = json!(0);
    fixture.write("complete-session-example.json", &receipt);
    fixture.assert_ineligible_completion("complete-session-example.json");
    let mut fixture = Fixture::new();
    fixture
        .events
        .push(json!({"type":"opened","session":"session-later","mode":"reconcile"}));
    fixture.events.push(
        json!({"type":"closed","outcome":"publishing","completed_at":"2026-10-24T00:00:00+00:00"}),
    );
    fixture.publish(false);
    let head = document::read(&fixture.root.join("head.json")).unwrap();
    let mut receipt = document::read(&fixture.root.join("complete-session-example.json")).unwrap();
    receipt["session"] = json!("session-later");
    receipt["journal_sha256"] = head["sha256"].clone();
    fixture.write("complete-session-later.json", &receipt);
    fixture.assert_ineligible_completion("complete-session-later.json");
}
