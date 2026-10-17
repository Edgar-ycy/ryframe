use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use regex::Regex;

use super::{
    policy::{Policy, ServiceImage},
    yaml::{self, YamlKey, YamlValue},
};

#[path = "workflow/actions.rs"]
mod actions;
#[path = "workflow/commands.rs"]
mod commands;

#[derive(Debug, Clone)]
pub(super) struct RunBlock {
    pub(super) path: PathBuf,
    pub(super) location: String,
    pub(super) source: String,
}

type ServiceIdentity = (String, String, String);
type ServiceImageMap = BTreeMap<ServiceIdentity, String>;

pub(crate) fn validate(directory: &Path, policy: &Policy) -> Vec<String> {
    let files = match workflow_files(directory) {
        Ok(files) if !files.is_empty() => files,
        Ok(_) => return vec![format!("没有找到工作流：{}", directory.display())],
        Err(error) => {
            return vec![format!(
                "无法读取工作流目录 {}：{error}",
                directory.display()
            )];
        }
    };
    let mut errors = Vec::new();
    let mut installed_tools = Vec::new();
    let mut references = Vec::new();
    let mut run_blocks = Vec::new();
    let mut service_images = ServiceImageMap::new();
    for path in files {
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                errors.push(format!("{}: 工作流无法读取：{error}", path.display()));
                continue;
            }
        };
        let (document, parse_errors) = load_workflow(&path, &source);
        errors.extend(parse_errors);
        let Some(document) = document else {
            continue;
        };
        let (jobs, job_errors) = workflow_jobs(&path, &document);
        errors.extend(job_errors);
        errors.extend(validate_environment_contexts(&path, &document, &jobs));
        let (images, image_errors) = collect_service_images(&path, &jobs);
        errors.extend(image_errors);
        service_images.extend(images);
        let scan = actions::scan(&path, &jobs, policy);
        errors.extend(scan.errors);
        installed_tools.extend(scan.installed_tools);
        references.extend(scan.references);
        run_blocks.extend(scan.run_blocks);
    }

    errors.extend(compare_service_images(policy, &service_images));
    for (name, version) in &policy.tools {
        if !installed_tools
            .iter()
            .any(|(installed_name, _)| installed_name == name)
        {
            errors.push(format!("工作流没有安装策略声明的工具 {name}@{version}"));
        }
    }
    errors.extend(commands::validate_run_commands(&run_blocks));
    if !references
        .iter()
        .any(|reference| reference.starts_with("actions/upload-artifact@"))
    {
        errors.push("工作流缺少供应链门禁：actions/upload-artifact".into());
    }
    errors
}

#[allow(dead_code)]
pub(crate) fn validate_service_images(path: &Path, source: &str) -> Vec<String> {
    let (document, errors) = load_workflow(path, source);
    let Some(document) = document else {
        return errors;
    };
    let (jobs, job_errors) = workflow_jobs(path, &document);
    let (_, image_errors) = collect_service_images(path, &jobs);
    let mut result = errors;
    result.extend(job_errors);
    result.extend(image_errors);
    result
}

#[allow(dead_code)]
pub(crate) fn validate_action_uses(path: &Path, source: &str, policy: &Policy) -> Vec<String> {
    actions::validate_action_uses(path, source, policy)
}

fn workflow_files(directory: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|value| value.to_str()),
                    Some("yml" | "yaml")
                )
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

pub(super) fn load_workflow(path: &Path, source: &str) -> (Option<YamlValue>, Vec<String>) {
    match yaml::parse(source) {
        Ok(value) if value.as_mapping().is_some() => (Some(value), Vec::new()),
        Ok(_) => (
            None,
            vec![format!("{}: 工作流根节点必须是对象", path.display())],
        ),
        Err(error) => (
            None,
            vec![format!(
                "{}: 工作流 YAML 无法安全解析：{error}",
                path.display()
            )],
        ),
    }
}

pub(super) fn workflow_jobs(
    path: &Path,
    document: &YamlValue,
) -> (Vec<(String, YamlValue)>, Vec<String>) {
    let Some(jobs) = document.get("jobs") else {
        return (
            Vec::new(),
            vec![format!("{}: jobs 必须是对象", path.display())],
        );
    };
    let Some(jobs) = jobs.as_mapping() else {
        return (
            Vec::new(),
            vec![format!("{}: jobs 必须是对象", path.display())],
        );
    };
    let mut valid = Vec::new();
    let mut errors = Vec::new();
    for (name, job) in jobs {
        let YamlKey::String(name) = name else {
            errors.push(format!(
                "{}: jobs 中的任务名称和配置必须分别是字符串、对象",
                path.display()
            ));
            continue;
        };
        if job.as_mapping().is_none() {
            errors.push(format!(
                "{}: jobs 中的任务名称和配置必须分别是字符串、对象",
                path.display()
            ));
            continue;
        }
        valid.push((name.clone(), job.clone()));
    }
    (valid, errors)
}

fn validate_environment_contexts(
    path: &Path,
    document: &YamlValue,
    jobs: &[(String, YamlValue)],
) -> Vec<String> {
    let mut errors = Vec::new();
    let mut scopes = vec![(
        "工作流 env".to_owned(),
        document.get("env"),
        [
            "runner", "job", "steps", "env", "needs", "strategy", "matrix",
        ]
        .as_slice(),
    )];
    for (name, job) in jobs {
        scopes.push((
            format!("job {name} env"),
            job.get("env"),
            ["runner", "job", "steps", "env"].as_slice(),
        ));
    }
    for (location, environment, forbidden) in scopes {
        let Some(environment) = environment else {
            continue;
        };
        let Some(environment) = environment.as_mapping() else {
            errors.push(format!("{}: {location} 必须是对象", path.display()));
            continue;
        };
        for (variable, value) in environment {
            let YamlKey::String(variable) = variable else {
                continue;
            };
            for name in forbidden {
                if contains_expression_reference(value, name) {
                    errors.push(format!(
                        "{}: {location}.{variable} 不能引用 {name} 上下文；请下沉到具体 step 的 env、with 或 run",
                        path.display()
                    ));
                }
            }
            if value.as_str().is_some_and(|value| {
                value.contains("${{") && hash_files_expression().is_match(value)
            }) {
                errors.push(format!(
                    "{}: {location}.{variable} 不能调用 hashFiles；请下沉到具体 step 的 env 或 with",
                    path.display()
                ));
            }
        }
    }
    errors
}

fn contains_expression_reference(value: &YamlValue, name: &str) -> bool {
    let Some(value) = value.as_str() else {
        return false;
    };
    if !value.contains("${{") {
        return false;
    }
    let pattern = format!(r"(?:^|[^A-Za-z0-9_]){}\s*(?:\.|\[)", regex::escape(name));
    Regex::new(&pattern).is_ok_and(|regex| regex.is_match(value))
}

fn hash_files_expression() -> Regex {
    Regex::new(r"\bhashFiles\s*\(").expect("固定 hashFiles 正则")
}

fn collect_service_images(
    path: &Path,
    jobs: &[(String, YamlValue)],
) -> (ServiceImageMap, Vec<String>) {
    let mut images = BTreeMap::new();
    let mut errors = Vec::new();
    for (job_name, job) in jobs {
        let Some(services) = job.get("services") else {
            continue;
        };
        let Some(services) = services.as_mapping() else {
            errors.push(format!(
                "{}: job {job_name} 的 services 必须是对象",
                path.display()
            ));
            continue;
        };
        for (service_name, service) in services {
            let YamlKey::String(service_name) = service_name else {
                errors.push(format!(
                    "{}: job {job_name} 的 service 名称和配置必须分别是字符串、对象",
                    path.display()
                ));
                continue;
            };
            if service.as_mapping().is_none() {
                errors.push(format!(
                    "{}: job {job_name} 的 service 名称和配置必须分别是字符串、对象",
                    path.display()
                ));
                continue;
            }
            let Some(image) = service.get("image") else {
                errors.push(format!(
                    "{}: job {job_name} 的 service {service_name} 缺少固定 image",
                    path.display()
                ));
                continue;
            };
            let Some(image) = image.as_str() else {
                errors.push(format!(
                    "{}: job {job_name} 的 service {service_name} 镜像未固定到 sha256 digest：{}",
                    path.display(),
                    display_untrusted(&format!("{image:?}"))
                ));
                continue;
            };
            images.insert(
                (
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    job_name.clone(),
                    service_name.clone(),
                ),
                image.to_owned(),
            );
            if !service_image_ref(image) {
                errors.push(format!(
                    "{}: job {job_name} 的 service {service_name} 镜像未固定到 sha256 digest：{}",
                    path.display(),
                    display_untrusted(image)
                ));
            }
        }
    }
    (images, errors)
}

fn compare_service_images(policy: &Policy, actual: &ServiceImageMap) -> Vec<String> {
    let expected = policy
        .service_images
        .iter()
        .map(service_identity)
        .collect::<BTreeMap<_, _>>();
    let mut errors = Vec::new();
    for (workflow, job, service) in expected.keys().filter(|key| !actual.contains_key(*key)) {
        errors.push(format!(
            "策略声明的服务镜像不存在于工作流：{workflow}/{job}/{service}"
        ));
    }
    for (workflow, job, service) in actual.keys().filter(|key| !expected.contains_key(*key)) {
        errors.push(format!(
            "工作流服务镜像未在策略声明：{workflow}/{job}/{service}"
        ));
    }
    for (identity, expected_image) in &expected {
        let Some(actual_image) = actual.get(identity) else {
            continue;
        };
        if expected_image == actual_image {
            continue;
        }
        let (workflow, job, service) = identity;
        errors.push(format!(
            "服务镜像引用漂移 {workflow}/{job}/{service}：期望 {}，实际 {}",
            display_untrusted(expected_image),
            display_untrusted(actual_image)
        ));
    }
    errors
}

fn service_identity(image: &ServiceImage) -> (ServiceIdentity, String) {
    (
        (
            image.workflow.clone(),
            image.job.clone(),
            image.service.clone(),
        ),
        image.image.clone(),
    )
}

fn service_image_ref(value: &str) -> bool {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[0-9a-f]{64}$")
        .expect("固定服务镜像正则")
        .is_match(value)
}

pub(super) fn display_untrusted(value: &str) -> String {
    if value.contains("${{")
        || value.to_ascii_lowercase().contains("secret")
        || value.to_ascii_lowercase().contains("token")
    {
        return "<已隐藏敏感引用>".into();
    }
    let mut characters = value.chars();
    let mut display = characters.by_ref().take(240).collect::<String>();
    if characters.next().is_some() {
        display.push('…');
    }
    display
}
