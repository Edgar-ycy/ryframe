use super::{dependency::Dependencies, model::*};
use crate::{Result, recovery::clone::native::source};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(super) struct Paths {
    pub backend: PathBuf,
    pub environment: PathBuf,
    pub output: PathBuf,
    pub binding: PathBuf,
}
pub(super) struct Context {
    pub binding: Value,
    pub frontend: PathBuf,
    pub runtime: PathBuf,
    pub outputs: BTreeMap<String, PathBuf>,
    pub input_guards: Vec<Value>,
    pub source_guards: Vec<SourceGuard>,
    pub tool: Value,
    pub secrets: Vec<String>,
    pub private: Value,
}
pub(super) struct SourceGuard {
    root: PathBuf,
    inventory: source::Inventory,
    states: Vec<Value>,
}
impl SourceGuard {
    pub(super) fn protect(
        &self,
    ) -> Result<crate::recovery::clone::native::target_verify::guard::Inputs> {
        crate::recovery::clone::native::target_verify::guard::Inputs::open(
            self.states
                .iter()
                .map(|state| path(&state["descriptor"]["path"]))
                .collect::<Result<Vec<_>>>()?,
        )
    }
    pub(super) fn capture(root: &Path, expected: &Value) -> Result<Self> {
        let inventory = source::capture(root)?;
        let mut snapshot = source::binding_value(&inventory.source)["snapshot"].clone();
        snapshot
            .as_object_mut()
            .ok_or("源码快照无效")?
            .remove("clean");
        check(snapshot == *expected, "浏览器源码与已登记生成来源不一致")?;
        let mut states = vec![];
        for file in &inventory.files {
            let mut guard = super::model::snapshot(&root.join(&file.path), MAX_BYTES)?;
            guard["state"]
                .as_object_mut()
                .ok_or("源码文件状态无效")?
                .remove("modified");
            states.push(guard);
        }
        Ok(Self {
            root: root.to_owned(),
            inventory,
            states,
        })
    }
    pub(super) fn unchanged(&self) -> Result<()> {
        for expected in &self.states {
            let mut current = snapshot(&path(&expected["descriptor"]["path"])?, MAX_BYTES)?;
            current["state"]
                .as_object_mut()
                .ok_or("源码文件状态无效")?
                .remove("modified");
            check(current == *expected, "浏览器期间源码曾被替换或修改")?;
        }
        check(
            source::capture(&self.root)? == self.inventory,
            "浏览器期间完整源码发生变化",
        )
    }
}
impl Context {
    pub(super) fn unchanged(&self) -> Result<()> {
        for g in &self.input_guards {
            unchanged(g)?;
        }
        for g in &self.source_guards {
            g.unchanged()?;
        }
        Ok(())
    }
    pub(super) fn output(&self, key: &str) -> Result<&Path> {
        self.outputs
            .get(key)
            .map(PathBuf::as_path)
            .ok_or_else(|| "浏览器输出未登记".into())
    }
}
pub(super) fn runtime_args(paths: &Paths) -> Value {
    json!({"backend":paths.backend,"environment":paths.environment,"output":paths.output})
}
pub(super) fn outputs(
    frontend: &Path,
    runtime: &Path,
    run_id: &str,
    server: &str,
) -> BTreeMap<String, PathBuf> {
    let prefix = format!("browser-{run_id}");
    let mut outputs = BTreeMap::new();
    for (name, suffix) in [
        ("login_budget", "login-budget.json"),
        ("intent", "intent.json"),
        ("result", "result.json"),
        ("failure", "failure.json"),
        ("browser_log", "check.log"),
        ("browser_process", "check-process"),
    ] {
        outputs.insert(name.into(), runtime.join(format!("{prefix}-{suffix}")));
    }
    for (name, kind) in [("report", "report"), ("results", "results")] {
        outputs.insert(
            name.into(),
            frontend.join(format!(
                ".local-tests/playwright-real/{kind}/business/{server}/{run_id}"
            )),
        );
    }
    if server == "preview" {
        for (name, suffix) in [
            ("build", "build"),
            ("build_verify_before", "build-verify-before"),
            ("build_verify_after", "build-verify-after"),
        ] {
            outputs.insert(
                format!("{name}_log"),
                runtime.join(format!("{prefix}-{suffix}.log")),
            );
            outputs.insert(
                format!("{name}_process"),
                runtime.join(format!("{prefix}-{suffix}-process")),
            );
        }
        outputs.insert(
            "build_receipt".into(),
            frontend.join("dist/.vite/restore-build.json"),
        );
        outputs.insert(
            "response_audit".into(),
            runtime.join(format!("{prefix}-preview-responses.json")),
        );
    }
    outputs
}
pub(super) fn output_set(
    runtime: &Path,
    run_id: &str,
    outputs: &BTreeMap<String, PathBuf>,
    fresh: bool,
) -> Result<()> {
    let prefix = format!("browser-{run_id}-");
    for entry in fs::read_dir(runtime)? {
        let path = entry?.path();
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
        {
            check(
                outputs.values().any(|p| *p == path),
                "浏览器目录包含未登记同run id产物",
            )?;
            check(!fresh, "浏览器run id已有产物，禁止重放")?;
        }
    }
    if fresh {
        for path in outputs.values() {
            safe(path)?;
            check(!path.try_exists()?, "浏览器run id已有产物，禁止重放")?;
        }
    }
    Ok(())
}
pub(super) fn input_guards(value: &Value) -> Result<Vec<Value>> {
    let mut descriptors = BTreeMap::new();
    collect(value, &mut descriptors);
    let mut guards = vec![];
    for (p, v) in descriptors {
        let g = snapshot(Path::new(&p), MAX_BYTES)?;
        check(g["descriptor"] == v, "浏览器输入与绑定摘要不一致")?;
        guards.push(g);
    }
    Ok(guards)
}
fn collect(value: &Value, descriptors: &mut BTreeMap<String, Value>) {
    if let Some(v) = value.as_object() {
        if v.len() == 3
            && v.contains_key("bytes")
            && v.contains_key("sha256")
            && v.get("path")
                .and_then(Value::as_str)
                .is_some_and(|p| Path::new(p).is_absolute())
        {
            descriptors.insert(
                v["path"].as_str().expect("已验证字符串").into(),
                value.clone(),
            );
        } else {
            for v in v.values() {
                collect(v, descriptors);
            }
        }
    } else if let Some(v) = value.as_array() {
        for v in v {
            collect(v, descriptors);
        }
    }
}
pub(super) fn make(
    api: &mut impl Dependencies,
    paths: &Paths,
    run_id: &str,
    server: &str,
    fresh: bool,
    closed: bool,
    read_only: bool,
) -> Result<Context> {
    check(
        regex::Regex::new(r"^[a-z0-9][a-z0-9-]{0,63}$")?.is_match(run_id)
            && ["dev", "preview"].contains(&server),
        "浏览器run id或server无效",
    )?;
    let base = super::binding::Base::load(api, paths, closed, read_only)?;
    let (binding, outputs) = base.binding(api, run_id, server, fresh)?;
    base.context(binding, outputs)
}
pub(super) fn validate_endpoint(value: &str) -> Result<url::Url> {
    let parsed = url::Url::parse(value)?;
    check(
        parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
            && parsed.port().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.path() == "/"
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "浏览器端点必须是已审阅本机HTTP地址",
    )?;
    Ok(parsed)
}
pub(super) fn secrets(private: &Value, bootstrap: &Value) -> Result<Vec<String>> {
    let mut secrets = std::collections::BTreeSet::new();
    for descriptor in bootstrap["secret_files"]
        .as_object()
        .ok_or("bootstrap缺少秘密绑定")?
        .values()
    {
        let path = path(&descriptor["path"])?;
        super::model::descriptor(descriptor, &path)?;
        let raw = fs::read_to_string(&path)?;
        if !raw.trim().is_empty() {
            secrets.insert(raw.trim().into());
        }
        for line in raw.lines() {
            if line.trim().to_lowercase().starts_with("password=") {
                let value = line.split_once('=').ok_or("密码文件行无效")?.1.trim();
                if !value.is_empty() {
                    secrets.insert(value.into());
                }
            }
        }
    }
    for (key, value) in private.as_object().ok_or("私有环境无效")? {
        if let Some(value) = value.as_str().filter(|s| !s.is_empty()) {
            let key = key.to_uppercase();
            if ["PASSWORD", "SECRET", "TOKEN", "ACCESS_KEY", "CREDENTIAL"]
                .iter()
                .any(|h| key.contains(h))
            {
                secrets.insert(value.into());
            }
            if ["URL", "URI", "ENDPOINT"].iter().any(|s| key.ends_with(s))
                && let Ok(url) = url::Url::parse(value)
                && (!url.username().is_empty() || url.password().is_some())
            {
                secrets.insert(value.into());
                for v in [Some(url.username()), url.password()].into_iter().flatten() {
                    if !v.is_empty() {
                        secrets.insert(
                            percent_encoding::percent_decode_str(v)
                                .decode_utf8()?
                                .into_owned(),
                        );
                    }
                }
            }
        }
    }
    let mut secrets = secrets.into_iter().collect::<Vec<_>>();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    Ok(secrets)
}
