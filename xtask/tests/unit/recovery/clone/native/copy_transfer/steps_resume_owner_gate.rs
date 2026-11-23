use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Mutex,
};

use super::*;
use crate::{
    Result,
    recovery::clone::native::{
        copy_transfer::{
            Runner,
            object_port::{ObjectInputs, ObjectPort, Side},
            proofs::Proofs,
            publication::Attempt,
            record::Prepared,
            runner::{run_guarded_steps, with_continuation_owner},
            steps::Outcome,
        },
        digest,
        export::transport::{CommandOutput, CommandSpec, Transport},
        lifecycle::stage::{StageRequest, StageRunner},
        resources::common,
        s3::{Configuration, Operation, Request, app_configuration},
        target_verify::guard::Generation,
    },
};

#[test]
fn continuation_checks_all_five_owners_once_after_hydration_in_each_mode() {
    let (copy, mut io) = CompletedCopy::with_io();
    let objects = OwnerInputs::new(&copy);
    let original_result = document::read(&copy.fixture.copy.join("result.json")).unwrap();
    let original_writes = io.writes.clone();
    for (mode, status) in [
        (Mode::Reconcile, "copy_reconciled"),
        (Mode::Resume, "data_steps_verified"),
    ] {
        let directories = owner_directories(&copy.fixture.copy);
        let probe = OwnerProbe::new(&copy, &objects, false);
        let result = continue_copy(&copy, &objects, &probe, mode, &mut io).unwrap();
        assert_eq!(result["status"], status);
        assert_eq!(io.writes, original_writes);
        assert_eq!(
            document::read(&copy.fixture.copy.join("result.json")).unwrap(),
            original_result
        );

        let directory = new_owner_directory(&copy.fixture.copy, &directories);
        let receipt = document::read(&directory.join("verified.json")).unwrap();
        let buckets = ["avatar", "config-packages", "exports", "imports", "uploads"];
        assert_eq!(probe.requests.lock().unwrap().as_slice(), &buckets);
        assert_eq!(receipt["status"], "target_owners_verified");
        assert_eq!(receipt["scope_id"], copy.plan["target_scope"]);
        assert_eq!(receipt["copy_plan_sha256"], copy.plan["plan_sha256"]);
        assert_eq!(receipt["generation_sha256"], probe.generation);
        assert_eq!(
            receipt["configuration_sha256"],
            digest::plan_hash(&json!({"configuration":objects.target_configuration})).unwrap()
        );
        assert_eq!(receipt["remote_writes"], 0);
        let owners = receipt["owners"].as_array().unwrap();
        assert_eq!(owners.len(), buckets.len());
        for (owner, bucket) in owners.iter().zip(buckets) {
            assert_eq!(owner["bucket"], bucket);
            assert_eq!(owner["key"], probe.key());
            let file = directory.join(owner["file"].as_str().unwrap());
            let summary = digest::file(&file).unwrap();
            assert_eq!(owner["bytes"], summary.bytes);
            assert_eq!(owner["sha256"], summary.sha256);
            assert_eq!(
                fs::read(&file).unwrap(),
                owner_body(probe.target_scope, bucket).as_bytes()
            );
        }
        assert!(!directory.join("failure.json").exists());
    }
}

#[test]
fn continuation_rejects_wrong_imports_owner_without_ledger_or_step_changes() {
    let (copy, mut io) = CompletedCopy::with_io();
    let objects = OwnerInputs::new(&copy);
    let directories = owner_directories(&copy.fixture.copy);
    let before = copy.saved_copy();
    let writes = io.writes.clone();
    let database_events = io.database_events.len();
    let object_batches = io.object_batches.len();
    let business_objects = copy.plan["objects"].as_array().unwrap();
    assert_eq!(business_objects.len(), 1);
    assert_eq!(business_objects[0]["bucket"], "uploads");
    let probe = OwnerProbe::new(&copy, &objects, true);
    let error = continue_copy(&copy, &objects, &probe, Mode::Reconcile, &mut io).unwrap_err();
    assert_eq!(error.to_string(), "对象桶 owner 不属于已登记 scope");
    assert_eq!(copy.saved_copy(), before);
    assert_eq!(io.writes, writes);
    assert_eq!(io.database_events.len(), database_events);
    assert_eq!(io.object_batches.len(), object_batches);
    assert_eq!(
        probe.requests.lock().unwrap().as_slice(),
        &["avatar", "config-packages", "exports", "imports"]
    );
    assert!(
        !copy
            .fixture
            .backend
            .join(".local-tests/target/initialize.lock")
            .exists()
    );
    assert!(!copy.fixture.copy.join("ledger/clone.lock").exists());
    let directory = new_owner_directory(&copy.fixture.copy, &directories);
    let failure = document::read(&directory.join("failure.json")).unwrap();
    assert_eq!(failure["status"], "target_owners_failed");
    assert_eq!(failure["remote_writes"], 0);
    assert!(!directory.join("verified.json").exists());
    assert_eq!(
        fs::read(directory.join("owner-imports.bin")).unwrap(),
        owner_body(probe.source_scope, "imports").as_bytes()
    );
}

fn continue_copy(
    copy: &CompletedCopy,
    objects: &OwnerInputs,
    probe: &OwnerProbe<'_>,
    mode: Mode,
    io: &mut FixtureIo,
) -> Result<Value> {
    let backend = &copy.fixture.backend;
    let root = &copy.fixture.copy;
    let mode_name = if mode == Mode::Reconcile {
        "reconcile"
    } else {
        "resume"
    };
    Runner.preflight(&StageRequest {
        backend,
        run_dir: &copy.run,
        stage: "copy",
        mode: mode_name,
        attempt: None,
    })?;
    let files = Files::bind(backend, &copy.manifest, &copy.manifest["source_export"])?;
    let saved = files.continuation("source_to_seed")?;
    let attempt = Attempt::new(mode, root);
    let result = with_continuation_owner(
        backend,
        root,
        &files,
        &saved,
        |owner| {
            owner.check()?;
            let initialized = common::read_bound(backend, files.initialized, "原目标初始化")?;
            let proofs = Proofs::open(
                &copy.source.export,
                &saved.declaration["evidence"],
                &copy.source.value["evidence"]["source_stopped"],
                files.initialized,
                &initialized,
            )?;
            let prepared = files.hydrate(
                &saved,
                Facts {
                    source_generation: &json!({"stopped":true}),
                    target_generation: &json!({"initialized":true}),
                    source_observation: &saved.record["source_observation"],
                    declaration: &saved.declaration,
                },
            )?;
            proofs.checkpoint()?;
            owner.check()?;
            assert_eq!(prepared.plan, copy.plan);
            Ok((prepared, proofs))
        },
        |owner, (prepared, proofs)| {
            let result =
                run_with_object_port(copy, objects, probe, &owner, (&prepared, &proofs), mode, io);
            let mut published = None;
            owner.finish_publishing(result, |(outcome, ledger)| {
                published = Some(attempt.publish_bound(backend, root, &prepared, outcome, ledger)?);
                Ok(())
            })?;
            Ok(published.unwrap())
        },
    );
    attempt.failure(backend, root, result, "continuation")
}

fn run_with_object_port(
    copy: &CompletedCopy,
    objects: &OwnerInputs,
    probe: &OwnerProbe<'_>,
    owner: &Generation,
    (prepared, proofs): (&Prepared, &Proofs),
    mode: Mode,
    io: &mut FixtureIo,
) -> Result<(Outcome, Value)> {
    let authority = owner.copy_session()?;
    let plan = Plan::from_verified(&prepared.plan)?;
    let object = ObjectPort::new(objects.bind(copy, prepared), probe)?;
    let target = copy.fixture.backend.join(".local-tests/target");
    let check = || {
        owner.protects(&target)?;
        prepared.check()?;
        proofs.checkpoint()?;
        probe.assert_unopened();
        Ok(())
    };
    run_guarded_steps(
        authority,
        &plan,
        mode,
        io,
        |_| object.verify_target_owners(&check).map(|_| ()),
        || Ok(()),
    )
}

struct OwnerInputs {
    source_configuration: Value,
    target_configuration: Value,
    captures: BTreeMap<String, BTreeMap<String, Value>>,
    proof_files: Value,
}

impl OwnerInputs {
    fn new(copy: &CompletedCopy) -> Self {
        let export = document::read(&copy.source.export.join("export.json")).unwrap();
        // 归属门禁只需要索引原导出键，已确认业务像由原步骤夹具提供。
        let captures = export["objects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                let bucket = row["bucket"].as_str().unwrap().to_owned();
                let entries = row["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| (entry["key"].as_str().unwrap().to_owned(), entry.clone()))
                    .collect();
                (bucket, entries)
            })
            .collect();
        Self {
            source_configuration: config(copy, "source"),
            target_configuration: config(copy, "target"),
            captures,
            proof_files: json!({}),
        }
    }

    fn bind<'a>(&'a self, copy: &'a CompletedCopy, prepared: &'a Prepared) -> ObjectInputs<'a> {
        let side = |name, scope, configuration| Side {
            scope,
            endpoint: copy.source.value[name]["object_endpoint"].as_str().unwrap(),
            region: "us-east-1",
            configuration,
        };
        ObjectInputs {
            backend: &copy.fixture.backend,
            target_configuration_root: &copy.fixture.backend,
            work: &copy.fixture.copy,
            export_root: &copy.source.export,
            source: side(
                "source",
                prepared.plan["source_scope"].as_str().unwrap(),
                &self.source_configuration,
            ),
            target: side(
                "target",
                prepared.plan["target_scope"].as_str().unwrap(),
                &self.target_configuration,
            ),
            export: &copy.manifest["source_export"],
            captures: &self.captures,
            proof_files: &self.proof_files,
            plan_sha256: prepared.plan["plan_sha256"].as_str().unwrap(),
            generation_sha256: prepared.record["generation_sha256"].as_str().unwrap(),
        }
    }
}

fn config(copy: &CompletedCopy, side: &str) -> Value {
    let root = &copy.fixture.backend;
    let directory = root
        .join(".local-tests")
        .join(format!("owner-{side}-config"));
    fs::create_dir(&directory).unwrap();
    let endpoint = copy.source.value[side]["object_endpoint"].as_str().unwrap();
    let value = format!(
        "[object_storage]\nendpoint={endpoint:?}\nregion=\"us-east-1\"\naccess_key=\"fixture-{side}-key\"\nsecret_key=\"fixture-{side}-secret\"\n"
    );
    fs::write(directory.join("app.test.toml"), value).unwrap();
    app_configuration::configuration_value(root, &directory, "test").unwrap()
}

struct OwnerProbe<'a> {
    root: &'a Path,
    target_scope: &'a str,
    source_scope: &'a str,
    target_configuration: Configuration,
    generation: Value,
    head: Vec<u8>,
    journal: Vec<u8>,
    wrong_imports: bool,
    requests: Mutex<Vec<String>>,
}

impl<'a> OwnerProbe<'a> {
    fn new(copy: &'a CompletedCopy, inputs: &OwnerInputs, wrong_imports: bool) -> Self {
        let root = &copy.fixture.copy;
        let saved = document::read(&root.join("session.json")).unwrap();
        let target = app_configuration::private_configuration(
            &copy.fixture.backend,
            &inputs.target_configuration,
        )
        .unwrap();
        Self {
            root,
            target_scope: copy.plan["target_scope"].as_str().unwrap(),
            source_scope: copy.plan["source_scope"].as_str().unwrap(),
            target_configuration: Configuration::read(&target).unwrap(),
            generation: saved["generation_sha256"].clone(),
            head: fs::read(root.join("ledger/head.json")).unwrap(),
            journal: fs::read(root.join("ledger/events.ndjson")).unwrap(),
            wrong_imports,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn assert_unopened(&self) {
        assert!(!self.root.join("ledger/clone.lock").exists());
        assert_eq!(
            fs::read(self.root.join("ledger/head.json")).unwrap(),
            self.head
        );
        assert_eq!(
            fs::read(self.root.join("ledger/events.ndjson")).unwrap(),
            self.journal
        );
    }

    fn key(&self) -> String {
        format!("{}/.ryframe-owner", self.target_scope)
    }
}

fn owner_body(scope: &str, bucket: &str) -> String {
    format!("ryframe-owner:v1:{scope}:object-storage:{bucket}")
}

impl Transport for OwnerProbe<'_> {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput> {
        panic!("目标 owner 核对不得启动外部命令")
    }

    fn s3(&self, request: Request) -> Result<Value> {
        assert!(
            request.configuration == self.target_configuration,
            "归属请求必须使用已登记的目标对象存储配置"
        );
        let Operation::Get {
            key,
            destination,
            if_match,
            version,
            max_bytes,
        } = request.operation
        else {
            panic!("目标 owner 核对只允许读取 owner 对象");
        };
        assert_eq!(key, self.key());
        assert!(if_match.is_none() && version.is_none());
        assert_eq!(max_bytes, 4096);
        let directory = destination.parent().unwrap();
        assert_eq!(directory.parent(), Some(self.root));
        assert!(
            directory
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("target-owners-")
        );
        let scope = if self.wrong_imports && request.bucket == "imports" {
            self.source_scope
        } else {
            self.target_scope
        };
        let contents = owner_body(scope, &request.bucket);
        self.requests.lock().unwrap().push(request.bucket);
        fs::write(destination, &contents)?;
        Ok(json!({"ContentLength":contents.len()}))
    }
}

fn owner_directories(root: &Path) -> BTreeSet<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("target-owners-")
        })
        .collect()
}

fn new_owner_directory(root: &Path, before: &BTreeSet<PathBuf>) -> PathBuf {
    let current = owner_directories(root);
    let mut added = current.difference(before);
    let directory = added.next().unwrap().clone();
    assert!(added.next().is_none());
    directory
}
