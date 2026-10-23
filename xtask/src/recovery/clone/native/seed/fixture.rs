use super::super::{
    json as document,
    lifecycle::{lock::RunLock, state},
    resources::common,
};
use crate::data::performance_identities::{plan, test_fixture::Fixture as Identities};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

pub(super) struct Fixture {
    pub(super) identities: Identities,
    pub(super) backend: PathBuf,
    pub(super) run: PathBuf,
    pub(super) old: PathBuf,
    pub(super) request: Value,
    pub(super) owner: Option<RunLock>,
}
impl Fixture {
    pub(super) fn new() -> Self {
        let mut identities = Identities::new();
        let backend = crate::workspace::root_dir();
        let run = identities.directory.clone();
        fs::create_dir(run.join("post-copy")).unwrap();
        let old = run.join("post-copy").join("runtime");
        Self::identity_files(&mut identities, &backend, &run, &old);
        fs::create_dir(run.join("target")).unwrap();
        document::write_new(
            &run.join("target/initialized.json"),
            &json!({"fixture":true}),
        )
        .unwrap();
        document::write_new(
            &run.join("manifest.json"),
            &json!({"initialized":common::binding(&run.join("target/initialized.json")).unwrap()}),
        )
        .unwrap();
        state::initialize(&run).unwrap();
        let mut fixture = Self {
            identities,
            backend,
            run,
            old,
            request: Value::Null,
            owner: None,
        };
        let copied = fixture.stage(
            "copy",
            "resume",
            Some(json!({"status":"data_steps_verified"})),
            None,
        );
        let generic = fixture.file("historical", &json!({"fixture":true}));
        let environment = fixture.file(
            "api-environment",
            &json!({"environment":{"APP_ENV":"test"}}),
        );
        let post = json!({"copy_stage_receipt":copied,"copy_result":generic,"ledger_head":generic,
            "backend_build":generic,"api_environment":environment,"node":{"fixture":true},
            "frontend_root":fixture.identities.plan["environment"]["frontend_dir"]});
        let descriptor = fixture.file("post-copy", &post);
        fixture.stage(
            "post-copy",
            "register",
            Some(json!({"status":"post_copy_registered","registration":descriptor})),
            None,
        );
        let verified = fixture.stage(
            "post-copy",
            "verify",
            Some(json!({"status":"post_copy_existing_data_verified",
            "registration":descriptor,"worker_must_remain_stopped":true})),
            None,
        );
        let executable = std::env::current_exe().unwrap();
        fixture.request = json!({"format_version":1,"kind":"devex-clone-seed-runtime",
            "run_manifest":common::binding(&fixture.run.join("manifest.json")).unwrap(),"post_copy":descriptor,
            "post_verify":verified,"identity_plan":common::binding(&fixture.run.join("identity-plan.json")).unwrap(),
            "identity_state":fixture.run.join("seed-runtime/identities"),
            "identity_tool":{"path":executable,"sha256":super::super::digest::file(&executable).unwrap().sha256},
            "api_process":common::binding(&fixture.old.join("api.json")).unwrap(),"node":post["node"]});
        fixture.file("seed-request", &fixture.request);
        fixture
    }
    fn identity_files(
        identities: &mut Identities,
        backend: &std::path::Path,
        run: &std::path::Path,
        old: &std::path::Path,
    ) {
        fs::rename(run.join("runtime"), old).unwrap();
        let config = old.join("config");
        fs::create_dir(&config).unwrap();
        let private_path = run.join("private.json");
        let mut private = document::read(&private_path).unwrap();
        fs::write(
            config.join("app.test.toml"),
            r#"scope_id = "native-fixture"
[cors]
allow_origins = ["http://127.0.0.1:4199"]
[database.primary]
host = "127.0.0.1"
port = 3306
username = "fixture"
password = "Fixture-Database!123"
database = "identity_control"
tls_mode = "required"
"#,
        )
        .unwrap();
        document::write_replace(
            &old.join("runtime.json"),
            &json!({"scope_id":"native-fixture","api_config_dir":config}),
        )
        .unwrap();
        document::write_new(
            &old.join("handoff.json"),
            &json!({"api_url":"http://127.0.0.1:18999"}),
        )
        .unwrap();
        private["runtime"] = json!({"directory":old,"receipt_sha256":common::binding(&old.join("runtime.json")).unwrap()["sha256"],
            "api_process_sha256":common::binding(&old.join("api.json")).unwrap()["sha256"]});
        document::write_replace(&private_path, &private).unwrap();
        identities.plan = plan::create(&private, backend, &private_path).unwrap();
        document::write_new(&run.join("identity-plan.json"), &identities.plan).unwrap();
    }
    pub(super) fn file(&self, name: &str, value: &Value) -> Value {
        let path = self.run.join(format!("{name}.json"));
        document::write_new(&path, value).unwrap();
        common::binding(&path).unwrap()
    }
    pub(super) fn stage(
        &mut self,
        stage: &str,
        mode: &str,
        result: Option<Value>,
        error: Option<&str>,
    ) -> Value {
        let (number, _) = state::begin(&self.run, stage, mode, &json!({})).unwrap();
        state::finish(&self.run, number, result.as_ref(), error).unwrap();
        state::load(&self.run, true).unwrap()["attempts"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["result"]
            .clone()
    }
    pub(super) fn begin_registration(&mut self) -> Value {
        let owner = RunLock::claim(&self.run).unwrap();
        let (number, attempt) =
            state::begin(&self.run, "seed-runtime", "register", &json!({})).unwrap();
        state::bind_controller(&self.run, number, &owner.owner, &attempt).unwrap();
        fs::create_dir(self.run.join("target/initialize.lock")).unwrap();
        let lock = self.run.join("target/initialize.lock");
        let (_, inode) =
            common::device_inode_for(&lock, &fs::symlink_metadata(&lock).unwrap()).unwrap();
        self.owner = Some(owner);
        json!({"request":self.run.join("seed-request.json"),"attempt":number,
            "controller":common::binding(&state::controller_path(&self.run,number)).unwrap(),"target_lock_inode":inode})
    }
    pub(super) fn finish_registration(&mut self, result: &Value) {
        let number = state::load(&self.run, true).unwrap()["attempts"]
            .as_array()
            .unwrap()
            .len() as u64;
        state::finish(&self.run, number, Some(result), None).unwrap();
        self.owner.take().unwrap().release().unwrap();
        fs::remove_dir(self.run.join("target/initialize.lock")).unwrap();
    }
    pub(super) fn protocol(&self, operation: &str, arguments: Value) -> crate::Result<Value> {
        let value = json!({"format_version":1,"kind":"ryframe-seed-evidence","run_dir":self.run,"operation":operation,"arguments":arguments});
        super::protocol::execute(&self.backend, &serde_json::to_vec(&value)?)
    }
    pub(super) fn register(&mut self) {
        let args = self.begin_registration();
        let result = self.protocol("register", args).unwrap();
        self.finish_registration(&result);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            owner.release().unwrap();
        }
    }
}
