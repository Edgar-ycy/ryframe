use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::json;

use crate::Result;

#[path = "import_samples/archive.rs"]
mod archive;
#[path = "import_samples/model.rs"]
mod model;
#[path = "import_samples/publish.rs"]
mod publish;

pub(crate) use archive::{inspect_template, render_workbook};
#[allow(unused_imports)]
pub(crate) use model::{
    BrowserReceipt, HEADERS, ImportSampleManifest, LARGE_PAYLOAD_BYTES, LARGE_PAYLOAD_NAME,
    MAX_IMPORT_BYTES, MAX_TEMPLATE_BYTES, ManifestFile, PerformanceRequest, TemplateReceipt,
    department_sha256, model, model_sha256, performance_username, valid_namespace, valid_sha256,
};
pub(crate) use publish::{create_new_directory, publish_new, read_template_file};

const MARKER: &str = "__import-samples";
const FAILURE: &str = r#"{"ok":false,"reason":"import_fixture_preparation_failed"}"#;
const MAX_REQUEST_BYTES: usize = 32_768;

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let mut arguments = env::args_os().skip(1);
    if arguments.next()?.as_os_str() != MARKER {
        return None;
    }
    let arguments = arguments.collect::<Vec<_>>();
    let operation = arguments
        .first()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let mut input = Vec::new();
    let read_failed = operation == "performance"
        && std::io::stdin()
            .take((MAX_REQUEST_BYTES as u64) + 1)
            .read_to_end(&mut input)
            .is_err();
    if read_failed {
        emit_failure();
        return Some(1);
    }
    let (code, output) = execute(&arguments, &input);
    println!("{output}");
    Some(code)
}

pub(crate) fn execute(arguments: &[OsString], input: &[u8]) -> (i32, String) {
    match execute_inner(arguments, input) {
        Ok(response) => (0, response),
        Err(ProtocolFailure::Parameter) => (2, FAILURE.to_owned()),
        Err(ProtocolFailure::Operation) => (1, FAILURE.to_owned()),
    }
}

fn execute_inner(
    arguments: &[OsString],
    input: &[u8],
) -> std::result::Result<String, ProtocolFailure> {
    let operation = arguments
        .first()
        .and_then(|value| value.to_str())
        .ok_or(ProtocolFailure::Parameter)?;
    match operation {
        "performance" => {
            if arguments.len() != 1 {
                return Err(ProtocolFailure::Parameter);
            }
            if input.len() > MAX_REQUEST_BYTES {
                return Err(ProtocolFailure::Operation);
            }
            let request: PerformanceRequest =
                serde_json::from_slice(input).map_err(|_| ProtocolFailure::Operation)?;
            let manifest = prepare_performance(&request).map_err(|_| ProtocolFailure::Operation)?;
            let response = json!({"ok": true, "manifest": manifest});
            serde_json::to_string(&response).map_err(|_| ProtocolFailure::Operation)
        }
        "browser" => {
            let request = parse_browser(&arguments[1..]).map_err(|_| ProtocolFailure::Parameter)?;
            let receipt = prepare_browser(
                &request.template,
                &request.template_sha256,
                &request.output,
                &request.username,
                request.large,
            )
            .map_err(|_| ProtocolFailure::Operation)?;
            success("fixture", receipt)
        }
        "template" => {
            let request =
                parse_template(&arguments[1..]).map_err(|_| ProtocolFailure::Parameter)?;
            let receipt = template_receipt(&request.template, &request.template_sha256)
                .map_err(|_| ProtocolFailure::Operation)?;
            success("receipt", receipt)
        }
        _ => Err(ProtocolFailure::Parameter),
    }
}

fn success<T: Serialize>(key: &str, value: T) -> std::result::Result<String, ProtocolFailure> {
    let object = match key {
        "fixture" => json!({"ok": true, "fixture": value}),
        "receipt" => json!({"ok": true, "receipt": value}),
        _ => unreachable!("固定成功响应字段"),
    };
    serde_json::to_string(&object).map_err(|_| ProtocolFailure::Operation)
}

pub(crate) fn prepare_performance(request: &PerformanceRequest) -> Result<ImportSampleManifest> {
    if !valid_namespace(&request.namespace)
        || !valid_sha256(&request.template_sha256)
        || !valid_sha256(&request.model_sha256)
        || request.model_sha256 != model_sha256()?
    {
        return Err("导入样本namespace或固定模型SHA无效".into());
    }
    if request.concurrency == 0
        || request.concurrency > 100
        || request.cycles == 0
        || request.cycles > 10_000
        || u32::from(request.concurrency) * u32::from(request.cycles) > 10_000
    {
        return Err("样本维度无效或单次准备超过10000个文件".into());
    }
    let content = read_template_file(&request.template, &request.template_sha256)?;
    let template = inspect_template(&content)?;
    create_new_directory(&request.directory)?;
    let mut files =
        Vec::with_capacity(usize::from(request.concurrency) * usize::from(request.cycles));
    for worker in 0..request.concurrency {
        for cycle in 0..request.cycles {
            let username = performance_username(&request.namespace, worker, cycle);
            let filename = format!("{worker}-{cycle}.xlsx");
            let rows = [[
                username.clone(),
                "性能导入".into(),
                format!("{username}@example.test"),
                String::new(),
                template.department.clone(),
            ]];
            let content = render_workbook(&content, &template, &rows, false)?;
            let published = publish_new(&request.directory.join(&filename), &content)?;
            files.push(ManifestFile {
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
        model: model(),
        model_sha256: request.model_sha256.clone(),
        concurrency: request.concurrency,
        cycles: request.cycles,
        files,
    })
}

pub(crate) fn prepare_browser(
    template_path: &Path,
    template_sha256: &str,
    output: &Path,
    username: &str,
    large: bool,
) -> Result<BrowserReceipt> {
    if username.len() != 15
        || !username.starts_with("import-")
        || !username[7..]
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err("测试用户名必须使用唯一 import-xxxxxxxx 形式".into());
    }
    let content = read_template_file(template_path, template_sha256)?;
    let template = inspect_template(&content)?;
    let department = template.department.clone();
    let rows = [
        [
            username.into(),
            "导入验收".into(),
            format!("{username}@example.test"),
            String::new(),
            department.clone(),
        ],
        [
            username.into(),
            "重复行".into(),
            format!("{username}@example.test"),
            String::new(),
            department,
        ],
        [
            format!("{username}-bad"),
            "无效部门".into(),
            format!("{username}-bad@example.test"),
            String::new(),
            "不存在的隔离部门".into(),
        ],
    ];
    let content = render_workbook(&content, &template, &rows, large)?;
    Ok(BrowserReceipt {
        published: publish_new(output, &content)?,
        template_sha256: template_sha256.to_owned(),
        username: username.to_owned(),
        rows: 3,
        large,
    })
}

pub(crate) fn template_receipt(path: &Path, sha256: &str) -> Result<TemplateReceipt> {
    let content = read_template_file(path, sha256)?;
    let template = inspect_template(&content)?;
    Ok(TemplateReceipt {
        template_sha256: sha256.to_owned(),
        department_sha256: department_sha256(&template.department),
        department_path: template.department,
    })
}

fn parse_browser(arguments: &[OsString]) -> Result<BrowserRequest> {
    let parsed = Arguments::parse(
        arguments,
        &["--template", "--template-sha256", "--output", "--username"],
        true,
    )?;
    let template = parsed.path("--template")?;
    let template_sha256 = parsed.text("--template-sha256")?;
    let output = parsed.path("--output")?;
    let username = parsed.text("--username")?;
    Ok(BrowserRequest {
        template,
        template_sha256,
        output,
        username,
        large: parsed.flag("--large"),
    })
}

fn parse_template(arguments: &[OsString]) -> Result<TemplateRequest> {
    let parsed = Arguments::parse(arguments, &["--template", "--template-sha256"], false)?;
    let template = parsed.path("--template")?;
    let template_sha256 = parsed.text("--template-sha256")?;
    Ok(TemplateRequest {
        template,
        template_sha256,
    })
}

struct BrowserRequest {
    template: PathBuf,
    template_sha256: String,
    output: PathBuf,
    username: String,
    large: bool,
}

struct TemplateRequest {
    template: PathBuf,
    template_sha256: String,
}

struct Arguments {
    values: BTreeMap<String, OsString>,
    large: bool,
}

impl Arguments {
    fn parse(arguments: &[OsString], value_flags: &[&str], allow_large: bool) -> Result<Self> {
        let mut values = BTreeMap::new();
        let mut large = false;
        let mut index = 0;
        while index < arguments.len() {
            let flag = arguments[index].to_str().ok_or("参数必须是 UTF-8")?;
            if flag == "--large" {
                if !allow_large || large {
                    return Err("包含未知或重复参数".into());
                }
                large = true;
                index += 1;
                continue;
            }
            if !value_flags.contains(&flag) || values.contains_key(flag) {
                return Err("包含未知或重复参数".into());
            }
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| format!("{flag} 缺少取值"))?;
            values.insert(flag.to_owned(), value.clone());
            index += 2;
        }
        if values.len() != value_flags.len() {
            return Err("缺少必填参数".into());
        }
        Ok(Self { values, large })
    }

    fn text(&self, name: &str) -> Result<String> {
        self.values
            .get(name)
            .ok_or_else(|| format!("缺少 {name}"))?
            .to_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| format!("{name} 必须是 UTF-8").into())
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        Ok(PathBuf::from(self.text(name)?))
    }

    fn flag(&self, _name: &str) -> bool {
        self.large
    }
}

fn emit_failure() {
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{FAILURE}");
}

#[derive(Debug)]
enum ProtocolFailure {
    Parameter,
    Operation,
}
