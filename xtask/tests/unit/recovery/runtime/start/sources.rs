use super::*;
use std::path::PathBuf;

struct Fixture {
    selected: RuntimeSources,
    execution: Value,
    backend: Document,
    frontend: Document,
    authority: Value,
}

impl Fixture {
    fn new(legacy: bool) -> Self {
        let fixture = crate::workspace::root_dir().join("isolated-source-fixture");
        let execution_root = fixture.join("execution");
        let frontend_root = fixture.join("frontend");
        let product = legacy.then(|| fixture.join("product"));
        let adapter = legacy.then_some("legacy-stable-readiness-b0-v1".to_owned());
        let (backend_product_sha, backend_execution_sha, frontend_sha) = if legacy {
            (
                crate::recovery::b0::BACKEND.to_owned(),
                crate::recovery::b0::COMMIT.to_owned(),
                "0087ea2ecf62530d042b9e52f5c950fb34c66d78".to_owned(),
            )
        } else {
            ("1".repeat(40), "1".repeat(40), "2".repeat(40))
        };
        let backend = build(execution_root.join(".local-tests/backend.json"));
        let frontend = build(frontend_root.join("dist/.vite/restore-build.json"));
        let execution = json!({
            "roots":{
                "source_backend":protocol_path(product.as_deref().unwrap_or(&execution_root)).unwrap(),
                "execution_backend":protocol_path(&execution_root).unwrap(),
                "frontend":protocol_path(&frontend_root).unwrap(),
            },
            "backend_product_sha":backend_product_sha,
            "backend_execution_sha":backend_execution_sha,
            "frontend_sha":frontend_sha,
            "builds":{"backend":document::binding(&backend),"frontend":document::binding(&frontend)},
            "adapter":adapter.as_ref().map(|value| json!({"contract":value})),
        });
        let authority = json!({
            "backend_product_sha":backend_product_sha,
            "backend_execution_sha":backend_execution_sha,
            "frontend_sha":frontend_sha,
            "backend_adapter_contract":adapter,
        });
        Self {
            selected: RuntimeSources {
                source_backend: execution_root,
                source_frontend: frontend_root,
                adapter_contract: adapter,
                product_backend: product,
            },
            execution,
            backend,
            frontend,
            authority,
        }
    }

    fn bind(&self) -> Result<Value> {
        bind(
            &self.selected,
            &self.execution,
            &self.backend,
            &self.frontend,
            &self.authority,
        )
    }
}

fn build(path: PathBuf) -> Document {
    let path = protocol_path(&path).unwrap();
    Document {
        path,
        raw: b"{}\n".to_vec(),
        value: json!({}),
    }
}

#[test]
fn ordinary_runtime_binds_the_same_backend_and_exact_two_receipts() {
    let mut fixture = Fixture::new(false);
    let roots = fixture.bind().unwrap();
    assert_eq!(
        roots,
        json!({
            "backend_product":fixture.execution["roots"]["source_backend"],
            "backend_execution":fixture.execution["roots"]["execution_backend"],
            "frontend":fixture.execution["roots"]["frontend"],
        })
    );
    assert_eq!(roots["backend_product"], roots["backend_execution"]);

    let backend = fixture.backend.clone();
    fixture.backend.path = fixture.backend.path.with_file_name("same-bytes-copy.json");
    assert!(fixture.bind().is_err());
    fixture.backend = backend;
    fixture.frontend.raw = b"{}\r".to_vec();
    assert!(fixture.bind().is_err());
    fixture.frontend.raw = b"{}\n".to_vec();
    fixture.execution["builds"]["backend"]["bytes"] = json!(1);
    assert!(fixture.bind().is_err());
}

#[test]
fn only_exact_b0_combination_permits_two_distinct_backend_roots() {
    let mut fixture = Fixture::new(true);
    let roots = fixture.bind().unwrap();
    assert_ne!(roots["backend_product"], roots["backend_execution"]);

    let product = fixture.selected.product_backend.clone();
    fixture.selected.product_backend = Some(fixture.selected.source_backend.clone());
    assert!(fixture.bind().is_err());
    fixture.selected.product_backend = product;
    fixture.authority["frontend_sha"] = json!("2".repeat(40));
    fixture.execution["frontend_sha"] = fixture.authority["frontend_sha"].clone();
    assert!(fixture.bind().is_err());

    let mut fixture = Fixture::new(false);
    fixture.authority["frontend_sha"] = json!(b0::HEAD);
    fixture.execution["frontend_sha"] = json!(b0::HEAD);
    assert!(fixture.bind().is_err());
    fixture.authority["frontend_sha"] = json!("2".repeat(40));
    fixture.execution["frontend_sha"] = json!("2".repeat(40));
    fixture.selected.product_backend = Some(fixture.selected.source_backend.clone());
    assert!(fixture.bind().is_err());
}

#[test]
fn refuses_changed_target_root_or_source_commit_before_build_validation() {
    let mut fixture = Fixture::new(false);
    let frontend = fixture.execution["roots"]["frontend"].clone();
    fixture.execution["roots"]["frontend"] =
        json!(fixture.selected.source_frontend.with_file_name("other"));
    assert!(fixture.bind().is_err());
    fixture.execution["roots"]["frontend"] = frontend;
    fixture.execution["backend_execution_sha"] = json!("3".repeat(40));
    assert!(fixture.bind().is_err());
}
