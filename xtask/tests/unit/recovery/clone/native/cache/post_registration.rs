use super::{common, digest, document, seed::history::Post, state, validate};
use crate::Result;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const COPY_REJECTED: &str = "复制后登记必须绑定最新复制成功阶段及完整账本head";
const BUILD_REJECTED: &str = "API产物必须属于本run已审计的产品构建";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    backend: PathBuf,
    run: PathBuf,
    manifest: Value,
    plan: Value,
    initial: Value,
    request: Value,
}

impl Fixture {
    fn new() -> Result<Self> {
        let parent = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&parent)?;
        let temporary = tempfile::Builder::new()
            .prefix("post-copy-registration-")
            .tempdir_in(parent)?;
        let root = fs::canonicalize(temporary.path())?;
        let backend = root.join("backend");
        let local = backend.join(".local-tests");
        let run = local.join("run");
        let copy = local.join("copy");
        let frontend = root.join("frontend");
        let module = frontend.join("scripts/browser-login-budget.mjs");
        fs::create_dir_all(frontend.join("scripts"))?;
        fs::write(&module, b"export const loginBudgetFixture = true;\n")?;
        let node = root.join("node-fixture");
        fs::write(&node, b"fixture tool binding only")?;
        fs::create_dir_all(&run)?;
        fs::create_dir_all(copy.join("ledger"))?;
        let environment = json!({"environment":{"APP_ENV":"test"}});
        let target_environment = write_bound(&local.join("target-environment.json"), &environment)?;
        let api_environment = write_bound(&local.join("api-environment.json"), &environment)?;
        let build = write_bound(&local.join("build.json"), &json!({"kind":"fixture-build"}))?;
        let bridge = write_bound(
            &local.join("bridge.json"),
            &json!({"build":tool_binding(&build)}),
        )?;
        let manifest = json!({
            "copy_directory":copy, "copy_stage":"source_to_seed",
            "target_environment":target_environment,
            "build_bridges":[tool_binding(&bridge)],
        });
        let run_manifest = write_bound(&run.join("manifest.json"), &manifest)?;
        state::initialize(&run)?;
        let (number, _) = state::begin(&run, "copy", "run", &json!({}))?;
        state::finish(
            &run,
            number,
            Some(&json!({"status":"data_steps_verified"})),
            None,
        )?;
        let credentials = local.join("administrator.toml");
        fs::write(
            &credentials,
            "[administrator]\ntenant_id = \"system\"\nusername = \"admin\"\npassword = \"test-fixture-only\"\n",
        )?;
        let reference = json!({"source":{"scope_id":"source-one"}});
        let dataset =
            json!({"plan_sha256":digest::plan_hash(&reference)?,"source_scope_id":"source-one"});
        let request = json!({
            "format_version":1, "kind":"devex-clone-post-copy",
            "run_manifest":run_manifest,
            "copy_stage_receipt":common::binding(&state::result_path(&run, number))?,
            "copy_result":write_bound(&copy.join("result.json"), &json!({"status":"data_steps_verified"}))?,
            "ledger_head":write_bound(&copy.join("ledger/head.json"), &json!({"sequence":2}))?,
            "api_environment":api_environment, "backend_build":build,
            "node":tool_binding(&common::binding(&node)?),
            "source_admin":{"subject_id":"9","tenant_id":"system","username":"admin","client_address":"198.19.20.1"},
            "administrator_credentials":common::binding(&credentials)?,
            "frontend_root":frontend,
            "pacing":{
                "contract":{},
                "bindings":{"login_budget_state":run.join("post-copy/login-budget.json")},
                "login_budget_module_sha256":digest::file(&module)?.sha256,
            },
            "reference_plan":write_bound(&local.join("reference.json"), &reference)?,
            "dataset":write_bound(&local.join("dataset.json"), &dataset)?,
        });
        Ok(Self {
            _temporary: temporary,
            root,
            backend,
            run,
            manifest,
            plan: json!({"copy_stage":"source_to_seed","source_scope":"source-one"}),
            initial: json!({"generation":{"selected":{"frontend_url":"http://127.0.0.1:4190"}}}),
            request,
        })
    }

    fn post(&self, name: &str, request: &Value) -> Result<Post> {
        let path = self.backend.join(".local-tests").join(name);
        Ok(Post {
            descriptor: write_bound(&path, request)?,
            request: document::read(&path)?,
            runtime: self.run.join("post-copy/runtime"),
        })
    }

    fn check(&self, post: &Post, expected_error: Option<&str>) -> Result<()> {
        let before = snapshot(&self.root)?;
        let result = validate(
            &self.backend,
            &self.run,
            &self.manifest,
            post,
            &self.plan,
            &self.initial,
        );
        assert_eq!(snapshot(&self.root)?, before);
        assert!(!self.run.join("post-copy.json").exists());
        assert!(!self.run.join("post-copy").exists());
        match expected_error {
            Some(error) => assert_eq!(result.unwrap_err().to_string(), error),
            None => result?,
        }
        Ok(())
    }
}

fn write_bound(path: &Path, value: &Value) -> Result<Value> {
    document::write_new(path, value)?;
    common::binding(path)
}

fn tool_binding(file: &Value) -> Value {
    json!({"path":file["path"],"sha256":file["sha256"]})
}

fn snapshot(root: &Path) -> Result<BTreeMap<PathBuf, Option<Value>>> {
    let mut entries = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let binding = if entry.file_type()?.is_dir() {
                directories.push(path.clone());
                None
            } else {
                Some(common::binding(&path)?)
            };
            entries.insert(path, binding);
        }
    }
    Ok(entries)
}

#[test]
fn source_to_seed_registration_requires_exact_copy_and_build_bindings() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.check(&fixture.post("request.json", &fixture.request)?, None)?;
    for (field, error) in [
        ("copy_stage_receipt", COPY_REJECTED),
        ("copy_result", COPY_REJECTED),
        ("ledger_head", COPY_REJECTED),
        ("backend_build", BUILD_REJECTED),
    ] {
        let mut request = fixture.request.clone();
        let path = fixture
            .backend
            .join(".local-tests")
            .join(format!("other-{field}.json"));
        // 相同字节位于另一个真实文件，验证错绑而不是缺失或摘要损坏。
        fs::copy(request[field]["path"].as_str().unwrap(), &path)?;
        request[field] = common::binding(&path)?;
        let post = fixture.post(&format!("request-{field}.json"), &request)?;
        fixture.check(&post, Some(error))?;
    }
    Ok(())
}

#[test]
fn source_to_seed_registration_does_not_fall_back_after_a_failed_copy() -> Result<()> {
    let fixture = Fixture::new()?;
    let post = fixture.post("request.json", &fixture.request)?;
    fixture.check(&post, None)?;
    let (number, _) = state::begin(&fixture.run, "copy", "resume", &json!({}))?;
    state::finish(&fixture.run, number, None, Some("FixtureCopyFailure"))?;
    fixture.check(&post, Some(COPY_REJECTED))
}
