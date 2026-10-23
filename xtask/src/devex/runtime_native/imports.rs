use super::{
    artifacts, configuration,
    failure::{Failure, NativeResult},
    process::Cancellation,
};
use crate::devex::import_samples::{self, ImportSampleManifest, PerformanceRequest};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Instant,
};

pub(crate) struct Samples {
    directory: PathBuf,
    files: BTreeMap<String, (String, String)>,
}
impl Samples {
    pub(crate) fn verify(
        directory: &Path,
        manifest: &ImportSampleManifest,
        request: &PerformanceRequest,
    ) -> NativeResult<Self> {
        if manifest.format_version != 1
            || manifest.namespace != request.namespace
            || manifest.template_sha256 != request.template_sha256
            || manifest.model_sha256 != request.model_sha256
            || configuration::hash(&json!(manifest.model))? != request.model_sha256
            || manifest.concurrency != request.concurrency
            || manifest.cycles != request.cycles
            || manifest.files.len()
                != usize::from(request.concurrency) * usize::from(request.cycles)
        {
            return Err(Failure::new("import_manifest_binding"));
        }
        artifacts::parents(directory)?;
        let root = directory
            .canonicalize()
            .map_err(|_| Failure::new("import_directory"))?;
        let mut files = BTreeMap::new();
        for file in &manifest.files {
            let (worker, cycle) = file
                .sample
                .split_once(':')
                .ok_or_else(|| Failure::new("import_sample_binding"))?;
            let worker: u16 = worker
                .parse()
                .map_err(|_| Failure::new("import_sample_binding"))?;
            let cycle: u16 = cycle
                .parse()
                .map_err(|_| Failure::new("import_sample_binding"))?;
            if worker >= request.concurrency
                || cycle >= request.cycles
                || file.sample != format!("{worker}:{cycle}")
                || file.filename != format!("{worker}-{cycle}.xlsx")
                || file.username
                    != import_samples::performance_username(&request.namespace, worker, cycle)
                || !configuration::digest(&file.sha256)
                || files
                    .insert(
                        file.sample.clone(),
                        (file.filename.clone(), file.sha256.clone()),
                    )
                    .is_some()
            {
                return Err(Failure::new("import_sample_binding"));
            }
        }
        let samples = Self {
            directory: root,
            files,
        };
        for key in samples.files.keys() {
            samples.read(key)?;
        }
        Ok(samples)
    }
    pub(crate) fn read(&self, sample: &str) -> NativeResult<Vec<u8>> {
        let (filename, digest) = self
            .files
            .get(sample)
            .ok_or_else(|| Failure::new("import_unknown_sample"))?;
        let path = self.directory.join(filename);
        if path
            .canonicalize()
            .map_err(|_| Failure::new("import_sample_path"))?
            != path
        {
            return Err(Failure::new("import_sample_path"));
        }
        let bytes = artifacts::read(&path, 16 * 1024 * 1024)?;
        if artifacts::sha(&bytes) != *digest {
            return Err(Failure::new("import_sample_changed"));
        }
        Ok(bytes)
    }
}

pub(crate) fn prepare(
    config: &Value,
    suite: &str,
    concurrency: u64,
    directory: &Path,
    cancel: &Cancellation,
) -> NativeResult<Option<Samples>> {
    if !config["contract"]["workloads"][suite]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row["job"]["kind"] == "import"))
    {
        return Ok(None);
    }
    cancel.check()?;
    let specification = &config["contract"]["import_samples"];
    if specification.as_object().is_none_or(|row| row.len() != 2)
        || specification["model_sha256"].as_str()
            != Some(
                import_samples::model_sha256()
                    .map_err(|_| Failure::new("import_model"))?
                    .as_str(),
            )
        || config["bindings"]["import_samples"]
            .as_object()
            .is_none_or(|row| row.len() != 1)
    {
        return Err(Failure::new("import_specification"));
    }
    let random = uuid::Uuid::new_v4().simple().to_string();
    let namespace = format!("{}{}", &random[..12], &random[24..]);
    let request = PerformanceRequest {
        namespace: namespace.clone(),
        directory: directory.join(format!("import-{namespace}")),
        template: PathBuf::from(
            config["bindings"]["import_samples"]["template"]
                .as_str()
                .ok_or_else(|| Failure::new("import_template"))?,
        ),
        template_sha256: specification["template_sha256"]
            .as_str()
            .ok_or_else(|| Failure::new("import_template"))?
            .into(),
        model_sha256: specification["model_sha256"]
            .as_str()
            .ok_or_else(|| Failure::new("import_model"))?
            .into(),
        concurrency: u16::try_from(concurrency).map_err(|_| Failure::new("import_dimensions"))?,
        cycles: u16::try_from(
            config["contract"]["cycles"]
                .as_u64()
                .ok_or_else(|| Failure::new("import_dimensions"))?,
        )
        .map_err(|_| Failure::new("import_dimensions"))?,
    };
    if !request.template.is_absolute() {
        return Err(Failure::new("import_template"));
    }
    let started = Instant::now();
    let mut manifest = None;
    let result = (|| {
        manifest = Some(
            generate(&request, cancel)
                .map_err(|_| Failure::new("import_sample_preparation_failed"))?,
        );
        cancel.check()?;
        Samples::verify(
            &request.directory,
            manifest
                .as_ref()
                .ok_or_else(|| Failure::new("import_manifest_binding"))?,
            &request,
        )
    })();
    artifacts::write(
        &directory.join("import-preparation.json"),
        &json!({"format_version":1, "namespace":namespace,
        "directory":request.directory, "template_sha256":request.template_sha256, "model_sha256":request.model_sha256,
        "duration_ms": super::milliseconds(started.elapsed()), "success":result.is_ok(),
        "failure":result.as_ref().err().map(|_| "import_sample_preparation_failed"), "manifest":manifest}),
    )?;
    result.map(Some)
}

fn generate(
    request: &PerformanceRequest,
    cancel: &Cancellation,
) -> NativeResult<ImportSampleManifest> {
    if !import_samples::valid_namespace(&request.namespace)
        || !import_samples::valid_sha256(&request.template_sha256)
        || request.model_sha256
            != import_samples::model_sha256().map_err(|_| Failure::new("import_model"))?
        || request.concurrency == 0
        || request.concurrency > 100
        || request.cycles == 0
        || request.cycles > 10000
        || u32::from(request.concurrency) * u32::from(request.cycles) > 10000
    {
        return Err(Failure::new("import_dimensions"));
    }
    let started = Instant::now();
    let content = import_samples::read_template_file(&request.template, &request.template_sha256)
        .map_err(|_| Failure::new("import_template"))?;
    let template =
        import_samples::inspect_template(&content).map_err(|_| Failure::new("import_template"))?;
    import_samples::create_new_directory(&request.directory)
        .map_err(|_| Failure::new("import_directory"))?;
    let mut files = Vec::new();
    for worker in 0..request.concurrency {
        for cycle in 0..request.cycles {
            cancel.check()?;
            if started.elapsed() > std::time::Duration::from_secs(300) {
                return Err(Failure::new("import_sample_timeout"));
            }
            let username = import_samples::performance_username(&request.namespace, worker, cycle);
            let rows = [[
                username.clone(),
                "性能导入".into(),
                format!("{username}@example.test"),
                String::new(),
                template.department.clone(),
            ]];
            let bytes = import_samples::render_workbook(&content, &template, &rows, false)
                .map_err(|_| Failure::new("import_render"))?;
            cancel.check()?;
            let filename = format!("{worker}-{cycle}.xlsx");
            let published = import_samples::publish_new(&request.directory.join(&filename), &bytes)
                .map_err(|_| Failure::new("import_publish_unknown"))?;
            files.push(import_samples::ManifestFile {
                sample: format!("{worker}:{cycle}"),
                filename,
                username,
                sha256: published.sha256,
            });
        }
    }
    Ok(ImportSampleManifest {
        format_version: 1,
        namespace: request.namespace.clone(),
        template_sha256: request.template_sha256.clone(),
        model: import_samples::model(),
        model_sha256: request.model_sha256.clone(),
        concurrency: request.concurrency,
        cycles: request.cycles,
        files,
    })
}
