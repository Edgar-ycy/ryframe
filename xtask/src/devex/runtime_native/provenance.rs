use super::{
    Input, artifacts, configuration,
    failure::{Failure, NativeResult},
    process::{self, Cancellation},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(crate) struct Guard {
    python: PathBuf,
    script: PathBuf,
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
        let specification = specification(config, input);
        let mut guard = match specification {
            Ok((python, request)) => Self {
                python,
                script: input.driver.join("tools/python/devex_provenance.py"),
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
        let mut receipt = None;
        let result = async {
            let arguments = vec![
                "-B".into(),
                "-X".into(),
                "utf8".into(),
                self.script.to_string_lossy().into_owned(),
            ];
            let result = process::json(
                &self.python,
                &arguments,
                &self.cwd,
                &self.request,
                Duration::from_secs(180),
                4 * 1024 * 1024,
                cancel,
            )
            .await?;
            if result["ok"] != true {
                return Err(Failure::new("provenance_verifier_result"));
            }
            let received = result["receipt"].clone();
            if received["format_version"] != 3 || received["kind"] != "devex-runtime-provenance" {
                return Err(Failure::new("provenance_verifier_result"));
            }
            receipt = Some(received.clone());
            if !self.before.is_null()
                && configuration::hash(&received)? != configuration::hash(&self.before)?
            {
                return Err(Failure::new("provenance_sample_changed"));
            }
            Ok(received)
        }
        .await;
        artifacts::write(
            &self.directory.join(format!("provenance-{phase}.json")),
            &json!({"format_version":1,"phase":phase,
            "duration_ms":super::milliseconds(started.elapsed()),"success":result.is_ok(),"failure":result.as_ref().err().map(|error|error.category),"receipt":receipt}),
        )?;
        result
    }
}

pub(crate) fn specification(config: &Value, input: &Input) -> NativeResult<(PathBuf, Value)> {
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
    if fields.len() != 6
        || !fields.contains_key("python")
        || !fields.contains_key("environment_document")
        || descriptors
            .iter()
            .any(|key| !fields.contains_key(*key) || !descriptor(&binding[*key]))
    {
        return Err(Failure::new("provenance_bindings"));
    }
    let python = PathBuf::from(
        binding["python"]
            .as_str()
            .ok_or_else(|| Failure::new("provenance_bindings"))?,
    );
    let environment = Path::new(
        binding["environment_document"]
            .as_str()
            .ok_or_else(|| Failure::new("provenance_bindings"))?,
    );
    if !python.is_absolute()
        || !environment.is_absolute()
        || !configuration::fingerprint(&input.driver_fingerprint)
        || !configuration::fingerprint(&input.runner_frontend_fingerprint)
        || config["bindings"]["source_fingerprints"]["runner_frontend"]
            != input.runner_frontend_fingerprint
    {
        return Err(Failure::new("provenance_bindings"));
    }
    let mut provenance = binding.clone();
    provenance
        .as_object_mut()
        .ok_or_else(|| Failure::new("provenance_bindings"))?
        .remove("python");
    Ok((
        python,
        json!({"provenance":provenance,"scope_id":config["bindings"]["scope_id"],
        "source_fingerprints":config["bindings"]["source_fingerprints"],"api_url":config["bindings"]["api_url"],
        "frontend_url":config["bindings"]["frontend_url"],"metrics_urls":config["bindings"]["metrics_urls"],
        "backend":input.backend,"frontend":input.frontend,"driver":input.driver,"runner_frontend":input.runner_frontend,
        "driver_fingerprint":input.driver_fingerprint,"runner_frontend_fingerprint":input.runner_frontend_fingerprint,
        "environment_sha256":config["contract"]["environment_sha256"]}),
    ))
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
