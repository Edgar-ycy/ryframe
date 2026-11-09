//! 读取阶段前后共用的显式收据、环境文档和端点绑定。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

use crate::{
    devex::runtime_native::{
        artifacts, configuration,
        failure::{Failure, NativeResult},
    },
    local_test_path::{
        LocalTestPathKind::{ExistingDirectory, ExistingFile},
        strip_local_test_prefix, validate_absolute_path, validate_local_test_path,
    },
    recovery::{
        clone::native::json as strict,
        inputs::{
            canonical_endpoint,
            document::{self, Document, MAX_JSON_BYTES},
        },
        monitoring::execution::ProtectedInputs,
        runtime::dataset_authority::commands::same_path,
    },
};

const RECEIPTS: [&str; 4] = [
    "runtime_receipt",
    "target_plan",
    "runtime_registration",
    "runtime_launch",
];
const BINDINGS: &str = "bindings";
const RUNTIME_RECEIPTS: &str = "runtime_receipts";
const RUNTIME_ROOTS: &str = "runtime_roots";
const RUNTIME_ENDPOINTS: &str = "runtime_endpoints";
const ENVIRONMENT: &str = "environment_document";

pub(super) struct Observation {
    pub(super) backend: PathBuf,
    pub(super) frontend: PathBuf,
    pub(super) driver: PathBuf,
    pub(super) runner_frontend: PathBuf,
    pub(super) runtime: Document,
    pub(super) receipts: Value,
    pub(super) environment: Value,
    receipt_guard: ProtectedInputs,
    environment_guard: ProtectedInputs,
}

impl Observation {
    pub(super) fn read(request: &Value) -> NativeResult<Self> {
        let [backend, frontend, driver, runner_frontend] = roots(request)?;
        let provenance = &request["provenance"];
        strict::exact(
            provenance,
            &[
                "runtime_receipt",
                "target_plan",
                "runtime_registration",
                "runtime_launch",
                "environment_document",
            ],
            "DevEx 运行来源绑定",
        )
        .map_err(|_| Failure::new(RUNTIME_RECEIPTS))?;
        let (runtime, receipts, receipt_guard) = receipts(provenance, &driver)?;
        require_runtime_root(&runtime.value["paths"]["backend_execution_root"], &backend)?;
        require_runtime_root(&runtime.value["paths"]["frontend_root"], &frontend)?;
        verify_endpoints(request, &runtime.value)?;
        let (environment, environment_guard) = read_environment(
            &backend,
            &provenance["environment_document"],
            &request["environment_sha256"],
        )?;
        let observed = Self {
            backend,
            frontend,
            driver,
            runner_frontend,
            runtime,
            receipts,
            environment,
            receipt_guard,
            environment_guard,
        };
        observed.assert_unchanged()?;
        Ok(observed)
    }

    pub(super) fn assert_unchanged(&self) -> NativeResult<()> {
        let receipts = self
            .receipt_guard
            .assert_unchanged()
            .map_err(|_| Failure::new(RUNTIME_RECEIPTS));
        let environment = self
            .environment_guard
            .assert_unchanged()
            .map_err(|_| Failure::new(ENVIRONMENT));
        match (receipts, environment) {
            (Ok(()), result) | (result, Ok(())) => result,
            (Err(receipts), Err(environment)) => Err(receipts.and(environment)),
        }
    }
}

fn roots(request: &Value) -> NativeResult<[PathBuf; 4]> {
    strict::exact(
        request,
        &[
            "backend",
            "frontend",
            "driver",
            "runner_frontend",
            "driver_fingerprint",
            "runner_frontend_fingerprint",
            "scope_id",
            "source_fingerprints",
            "api_url",
            "frontend_url",
            "metrics_urls",
            "environment_sha256",
            "provenance",
        ],
        "DevEx 来源请求",
    )
    .map_err(|_| Failure::new(BINDINGS))?;
    let fingerprints = strict::exact(
        &request["source_fingerprints"],
        &["backend", "frontend", "runner_frontend"],
        "DevEx 业务与执行来源摘要",
    )
    .map_err(|_| Failure::new(BINDINGS))?;
    if !fingerprints
        .values()
        .chain([
            &request["driver_fingerprint"],
            &request["runner_frontend_fingerprint"],
        ])
        .all(|value| value.as_str().is_some_and(configuration::fingerprint))
        || request["runner_frontend_fingerprint"] != fingerprints["runner_frontend"]
        || !request["environment_sha256"]
            .as_str()
            .is_some_and(configuration::digest)
    {
        return Err(Failure::new(BINDINGS));
    }
    Ok([
        directory(&request["backend"], BINDINGS)?,
        directory(&request["frontend"], BINDINGS)?,
        directory(&request["driver"], BINDINGS)?,
        directory(&request["runner_frontend"], BINDINGS)?,
    ])
}

fn directory(value: &Value, stage: &'static str) -> NativeResult<PathBuf> {
    let path = value
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| Failure::new(stage))?;
    validate_absolute_path(&path, ExistingDirectory).map_err(|_| Failure::new(stage))?;
    Ok(path.components().collect())
}

fn receipts(provenance: &Value, driver: &Path) -> NativeResult<(Document, Value, ProtectedInputs)> {
    let paths = RECEIPTS
        .into_iter()
        .map(|name| {
            let binding = document::binding_shape(&provenance[name], "DevEx 原运行收据")
                .map_err(|_| Failure::new(RUNTIME_RECEIPTS))?;
            let path = binding.path.components().collect::<PathBuf>();
            if file_bytes(&path, driver, RUNTIME_RECEIPTS)? != binding.bytes {
                return Err(Failure::new(RUNTIME_RECEIPTS));
            }
            Ok((name, path))
        })
        .collect::<NativeResult<Vec<_>>>()?;
    let guard = ProtectedInputs::open(paths.iter().map(|(_, path)| path.clone()))
        .map_err(|_| Failure::new(RUNTIME_RECEIPTS))?;
    let mut selected = Map::new();
    let mut runtime = None;
    for (name, path) in paths {
        let read = document::read(&path, driver, "DevEx 原运行收据")
            .map_err(|_| Failure::new(RUNTIME_RECEIPTS))?;
        let binding = document::binding(&read);
        if binding != provenance[name] {
            return Err(Failure::new(RUNTIME_RECEIPTS));
        }
        selected.insert(name.to_owned(), binding);
        if name == "runtime_receipt" {
            runtime = Some(read);
        }
    }
    let runtime = runtime.ok_or_else(|| Failure::new(RUNTIME_RECEIPTS))?;
    Ok((runtime, Value::Object(selected), guard))
}

fn file_bytes(path: &Path, backend: &Path, stage: &'static str) -> NativeResult<u64> {
    validate_local_test_path(path, backend, ExistingFile).map_err(|_| Failure::new(stage))?;
    let size = fs::symlink_metadata(path)
        .map_err(|_| Failure::new(stage))?
        .len();
    if size == 0 || size > MAX_JSON_BYTES {
        return Err(Failure::new(stage));
    }
    Ok(size)
}

fn require_runtime_root(value: &Value, expected: &Path) -> NativeResult<()> {
    let recorded = directory(value, RUNTIME_ROOTS)?;
    if !same_path(&recorded, expected).map_err(|_| Failure::new(RUNTIME_ROOTS))? {
        return Err(Failure::new(RUNTIME_ROOTS));
    }
    Ok(())
}

fn read_environment(
    backend: &Path,
    path: &Value,
    expected_sha: &Value,
) -> NativeResult<(Value, ProtectedInputs)> {
    let path = path
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| Failure::new(ENVIRONMENT))?;
    let path = path.components().collect::<PathBuf>();
    let allowed = backend.join(".local-tests/devex");
    if strip_local_test_prefix(&path, &allowed).is_none_or(|path| path.as_os_str().is_empty()) {
        return Err(Failure::new(ENVIRONMENT));
    }
    let length = file_bytes(&path, backend, ENVIRONMENT)?;
    let guard = ProtectedInputs::open([path.clone()]).map_err(|_| Failure::new(ENVIRONMENT))?;
    let bytes = artifacts::read(&path, MAX_JSON_BYTES).map_err(|_| Failure::new(ENVIRONMENT))?;
    let sha = artifacts::sha(&bytes);
    if bytes.len() as u64 != length
        || expected_sha.as_str() != Some(sha.as_str())
        || !std::str::from_utf8(&bytes).is_ok_and(|text| !text.trim().is_empty())
    {
        return Err(Failure::new(ENVIRONMENT));
    }
    Ok((json!({"path":path, "bytes":length, "sha256":sha}), guard))
}

fn endpoint(value: &Value, path: &str) -> NativeResult<String> {
    canonical_endpoint(value, path, "DevEx 已绑定本机端点")
        .map_err(|_| Failure::new(RUNTIME_ENDPOINTS))
}

fn verify_endpoints(request: &Value, runtime: &Value) -> NativeResult<()> {
    let ready = &runtime["endpoints"];
    let api_ready = endpoint(&ready["api"], "/readyz")?;
    let worker_ready = endpoint(&ready["worker"], "/readyz")?;
    let api = api_ready
        .strip_suffix("/readyz")
        .ok_or_else(|| Failure::new(RUNTIME_ENDPOINTS))?;
    let worker = worker_ready
        .strip_suffix("/readyz")
        .ok_or_else(|| Failure::new(RUNTIME_ENDPOINTS))?;
    let metrics = &request["metrics_urls"];
    strict::exact(metrics, &["api", "worker"], "DevEx 指标端点")
        .map_err(|_| Failure::new(RUNTIME_ENDPOINTS))?;
    if !request["scope_id"]
        .as_str()
        .is_some_and(|scope| !scope.is_empty())
        || request["scope_id"] != runtime["restore"]["scope_id"]
        || endpoint(&request["api_url"], "")? != api
        || endpoint(&request["frontend_url"], "")? != endpoint(&ready["frontend"], "")?
        || endpoint(&metrics["api"], "/metrics")? != format!("{api}/metrics")
        || endpoint(&metrics["worker"], "/metrics")? != format!("{worker}/metrics")
    {
        return Err(Failure::new(RUNTIME_ENDPOINTS));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _work: tempfile::TempDir,
        request: Value,
        environment: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let local = crate::workspace::root_dir().join(".local-tests");
            fs::create_dir_all(&local).unwrap();
            let work = tempfile::tempdir_in(local).unwrap();
            let root = work.path().components().collect::<PathBuf>();
            let backend = root.join("execution");
            let frontend = root.join("frontend");
            let driver = root.join("driver");
            let runner_frontend = root.join("runner");
            let environment_root = backend.join(".local-tests").join("devex");
            for directory in [
                environment_root.clone(),
                frontend.clone(),
                driver.join(".local-tests"),
                runner_frontend.clone(),
            ] {
                fs::create_dir_all(directory).unwrap();
            }
            let environment = environment_root.join("environment.md");
            fs::write(&environment, b"isolated operator description\n").unwrap();
            let runtime = json!({
                "paths":{"backend_execution_root":backend,"frontend_root":frontend},
                "restore":{"scope_id":"devex-case"},
                "endpoints":{"api":"http://127.0.0.1:80/readyz","worker":"https://[::1]:443/readyz",
                    "frontend":"http://127.0.0.1:4173"},
            });
            let mut provenance = json!({"environment_document":environment});
            for name in RECEIPTS {
                let path = driver.join(".local-tests").join(format!("{name}.json"));
                let value = if name == "runtime_receipt" {
                    &runtime
                } else {
                    &Value::Null
                };
                let raw = if value.is_null() {
                    b"{}".to_vec()
                } else {
                    serde_json::to_vec(value).unwrap()
                };
                fs::write(&path, &raw).unwrap();
                provenance[name] =
                    json!({"path":path,"bytes":raw.len(),"sha256":artifacts::sha(&raw)});
            }
            let fingerprint = format!("sha256:{}", "a".repeat(64));
            let request = json!({
                "backend":backend,"frontend":frontend,"driver":driver,"runner_frontend":runner_frontend,
                "driver_fingerprint":fingerprint,"runner_frontend_fingerprint":fingerprint,
                "scope_id":"devex-case","source_fingerprints":{
                    "backend":fingerprint,"frontend":fingerprint,"runner_frontend":fingerprint,
                },
                "api_url":"http://127.0.0.1:80","frontend_url":"http://127.0.0.1:4173",
                "metrics_urls":{"api":"http://127.0.0.1:80/metrics","worker":"https://[::1]:443/metrics"},
                "environment_sha256":artifacts::sha(&fs::read(&environment).unwrap()),"provenance":provenance,
            });
            Self {
                _work: work,
                request,
                environment,
            }
        }
    }

    fn category(request: &Value) -> &'static str {
        Observation::read(request)
            .err()
            .expect("无效请求不能建立读取窗口")
            .category
    }

    #[test]
    fn devex_inputs_preserve_explicit_receipts_and_canonical_default_ports() {
        let fixture = Fixture::new();
        let observed = Observation::read(&fixture.request).unwrap();
        assert_eq!(json!(observed.backend), fixture.request["backend"]);
        assert_eq!(json!(observed.frontend), fixture.request["frontend"]);
        assert_eq!(json!(observed.driver), fixture.request["driver"]);
        assert_eq!(
            json!(observed.runner_frontend),
            fixture.request["runner_frontend"]
        );
        assert_eq!(observed.runtime.value["restore"]["scope_id"], "devex-case");
        assert_eq!(observed.receipts.as_object().unwrap().len(), RECEIPTS.len());
        for name in RECEIPTS {
            assert_eq!(observed.receipts[name], fixture.request["provenance"][name]);
        }
        assert_eq!(observed.environment["path"], json!(fixture.environment));
        assert_eq!(
            observed.environment["sha256"],
            fixture.request["environment_sha256"]
        );
        observed.assert_unchanged().unwrap();
    }

    #[test]
    fn devex_inputs_reject_changed_binding_roots_and_wrong_metrics_path() {
        let fixture = Fixture::new();
        let mut request = fixture.request.clone();
        request["unexpected"] = Value::Null;
        assert_eq!(category(&request), BINDINGS);
        request = fixture.request.clone();
        request["provenance"]["target_plan"]["sha256"] = json!("b".repeat(64));
        assert_eq!(category(&request), RUNTIME_RECEIPTS);
        request = fixture.request.clone();
        request["backend"] = request["driver"].clone();
        assert_eq!(category(&request), RUNTIME_ROOTS);
        request = fixture.request.clone();
        request["metrics_urls"]["api"] = json!("http://127.0.0.1:80/api/v1/monitor/metrics");
        assert_eq!(category(&request), RUNTIME_ENDPOINTS);
        request = fixture.request;
        request["scope_id"] = json!("foreign-scope");
        assert_eq!(category(&request), RUNTIME_ENDPOINTS);
    }

    #[test]
    fn devex_environment_rejects_invalid_utf8_and_detects_post_read_drift() {
        let fixture = Fixture::new();
        let observed = Observation::read(&fixture.request).unwrap();
        match fs::write(&fixture.environment, b"changed operator text") {
            Ok(()) => assert_eq!(
                observed.assert_unchanged().unwrap_err().category,
                ENVIRONMENT
            ),
            Err(error) => {
                assert!(
                    error.kind() == std::io::ErrorKind::PermissionDenied
                        || (cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33))),
                    "{error} ({:?})",
                    error.raw_os_error()
                );
                observed.assert_unchanged().unwrap();
            }
        }
        drop(observed);
        let mut request = fixture.request;
        let bad = [0xff];
        fs::write(&fixture.environment, bad).unwrap();
        request["environment_sha256"] = json!(artifacts::sha(&bad));
        assert_eq!(category(&request), ENVIRONMENT);
    }
}
