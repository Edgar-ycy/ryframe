//! 用同一冻结工具只读核对测量前后的运行来源。
use super::{
    Input, artifacts, configuration,
    failure::{Failure, NativeResult},
    process::{self, Cancellation},
};
use crate::devex::job_evidence::artifact::{self, Binding};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

mod inputs;
mod sources;

pub(super) const MARKER: &str = "__devex-runtime-provenance";
const LIMIT: usize = 4 * 1024 * 1024;

pub(crate) struct Guard {
    tool: Binding,
    cwd: PathBuf,
    request: Value,
    before: Value,
    directory: PathBuf,
}

impl Guard {
    pub(crate) async fn before(
        config: &Value,
        input: &Input,
        directory: &Path,
        cancel: &Cancellation,
    ) -> NativeResult<Self> {
        let mut guard = match specification(config, input) {
            Ok(request) => Self {
                tool: input.native_tool.clone(),
                cwd: input.driver.clone(),
                request,
                before: Value::Null,
                directory: directory.into(),
            },
            Err(error) => {
                artifacts::write(
                    &directory.join("provenance-before.json"),
                    &json!({"format_version":1,"phase":"before","duration_ms":0,"success":false,"failure":error.category}),
                )?;
                return Err(error);
            }
        };
        guard.before = guard.verify("before", cancel).await?;
        Ok(guard)
    }

    pub(crate) async fn after(&mut self, cancel: &Cancellation) -> NativeResult<()> {
        self.verify("after", cancel).await.map(|_| ())
    }

    async fn verify(&self, phase: &str, cancel: &Cancellation) -> NativeResult<Value> {
        let started = Instant::now();
        let result = self.observe(cancel).await;
        phase_receipt(
            &self.directory,
            phase,
            started.elapsed(),
            &self.before,
            result,
        )
    }

    async fn observe(&self, cancel: &Cancellation) -> NativeResult<Value> {
        check_tool(&self.tool)?;
        let arguments = [MARKER.to_owned()];
        let transport = json!({"request":self.request,"native_tool":self.tool});
        let result = process::json(
            &self.tool.path,
            &arguments,
            &self.cwd,
            &transport,
            Duration::from_secs(180),
            LIMIT,
            cancel,
        )
        .await?;
        check_tool(&self.tool)?;
        Ok(result)
    }
}

pub(super) fn phase_receipt(
    directory: &Path,
    phase: &str,
    elapsed: Duration,
    before: &Value,
    response: NativeResult<Value>,
) -> NativeResult<Value> {
    let mut receipt = None;
    let result = response.and_then(|result| {
        let received = &result["receipt"];
        if result["ok"] != true
            || received["format_version"] != 3
            || received["kind"] != "devex-runtime-provenance"
        {
            return Err(Failure::new("provenance_verifier_result"));
        }
        receipt = Some(received.clone());
        if !before.is_null() && configuration::hash(received)? != configuration::hash(before)? {
            return Err(Failure::new("provenance_sample_changed"));
        }
        Ok(received.clone())
    });
    artifacts::write(
        &directory.join(format!("provenance-{phase}.json")),
        &json!({"format_version":1,"phase":phase,"duration_ms":super::milliseconds(elapsed),
            "success":result.is_ok(),"failure":result.as_ref().err().map(|error|error.category),
            "receipt":receipt}),
    )?;
    result
}

fn check_tool(expected: &Binding) -> NativeResult<()> {
    if artifact::current_binding().ok().as_ref() != Some(expected) {
        return Err(Failure::new("native_tool_binding"));
    }
    Ok(())
}

pub(super) fn child(arguments: usize, reader: impl Read) -> i32 {
    match child_request(arguments, reader) {
        Ok(receipt) => {
            println!("{}", json!({"ok":true,"receipt":receipt}));
            0
        }
        Err(error) => {
            println!("{}", json!({"ok":false,"stage":error.category}));
            1
        }
    }
}

fn child_request(arguments: usize, reader: impl Read) -> NativeResult<Value> {
    if arguments != 1 {
        return Err(Failure::new("provenance_bindings"));
    }
    let mut bytes = Vec::new();
    reader
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::new("provenance_bindings"))?;
    if bytes.is_empty() || bytes.len() > LIMIT {
        return Err(Failure::new("provenance_bindings"));
    }
    let protocol = crate::recovery::clone::native::json::parse_fragment(
        std::str::from_utf8(&bytes).map_err(|_| Failure::new("provenance_bindings"))?,
        "DevEx 运行来源协议",
    )
    .map_err(|_| Failure::new("provenance_bindings"))?;
    if protocol.as_object().is_none_or(|fields| {
        fields.len() != 2 || !fields.contains_key("request") || !fields.contains_key("native_tool")
    }) {
        return Err(Failure::new("provenance_bindings"));
    }
    let tool: Binding = serde_json::from_value(protocol["native_tool"].clone())
        .map_err(|_| Failure::new("native_tool_binding"))?;
    check_tool(&tool)?;
    let receipt = verify_request(&protocol["request"])?;
    check_tool(&tool)?;
    Ok(receipt)
}

fn verify_request(request: &Value) -> NativeResult<Value> {
    let input = inputs::Observation::read(request)?;
    let sources = sources::Sources::read(request, &input)?;
    let runtime = crate::recovery::runtime::performance::observe(
        &input.driver,
        &input.backend,
        &input.frontend,
        &input.receipts,
    )
    .map_err(|_| Failure::new("runtime_generation"));
    let after = sources.assert_unchanged(request, &input);
    let runtime = super::merge(runtime, [after, input.assert_unchanged()])?;
    Ok(json!({
        "format_version":3,"kind":"devex-runtime-provenance","scope_id":request["scope_id"],
        "roots":{"backend_product":sources.product_backend,"backend_execution":input.backend,
            "frontend":input.frontend,"driver":input.driver,"runner_frontend":input.runner_frontend},
        "sources":sources.snapshots,"source_fingerprints":request["source_fingerprints"],
        "execution_sources":sources.execution_sources,"receipts":input.receipts,
        "runtime":runtime,"processes":runtime["verification"]["processes"],
        "environment_document":input.environment,"environment_evidence":"operator_declared_document",
        "api_url":request["api_url"],"frontend_url":request["frontend_url"],
        "metrics_urls":request["metrics_urls"],
    }))
}

pub(crate) fn specification(config: &Value, input: &Input) -> NativeResult<Value> {
    let binding = &config["bindings"]["provenance"];
    let descriptors = [
        "runtime_receipt",
        "target_plan",
        "runtime_registration",
        "runtime_launch",
    ];
    let fields = binding
        .as_object()
        .ok_or_else(|| Failure::new("provenance_bindings"))?;
    if fields.len() != 5
        || !fields.contains_key("environment_document")
        || descriptors
            .iter()
            .any(|key| !fields.contains_key(*key) || !descriptor(&binding[*key]))
    {
        return Err(Failure::new("provenance_bindings"));
    }
    let environment = Path::new(
        binding["environment_document"]
            .as_str()
            .ok_or_else(|| Failure::new("provenance_bindings"))?,
    );
    if !environment.is_absolute()
        || !configuration::fingerprint(&input.driver_fingerprint)
        || !configuration::fingerprint(&input.runner_frontend_fingerprint)
        || config["bindings"]["source_fingerprints"]["runner_frontend"]
            != input.runner_frontend_fingerprint
    {
        return Err(Failure::new("provenance_bindings"));
    }
    Ok(
        json!({"provenance":binding,"scope_id":config["bindings"]["scope_id"],
        "source_fingerprints":config["bindings"]["source_fingerprints"],"api_url":config["bindings"]["api_url"],
        "frontend_url":config["bindings"]["frontend_url"],"metrics_urls":config["bindings"]["metrics_urls"],
        "backend":input.backend,"frontend":input.frontend,"driver":input.driver,"runner_frontend":input.runner_frontend,
        "driver_fingerprint":input.driver_fingerprint,"runner_frontend_fingerprint":input.runner_frontend_fingerprint,
        "environment_sha256":config["contract"]["environment_sha256"]}),
    )
}
fn descriptor(value: &Value) -> bool {
    value.as_object().is_some_and(|row| {
        row.len() == 3
            && row.contains_key("bytes")
            && row.contains_key("path")
            && row.contains_key("sha256")
    }) && value["path"]
        .as_str()
        .is_some_and(|value| Path::new(value).is_absolute())
        && value["bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0 && bytes <= 9_007_199_254_740_991)
        && value["sha256"].as_str().is_some_and(configuration::digest)
}
