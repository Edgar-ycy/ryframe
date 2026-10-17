use std::{collections::BTreeSet, fs, path::Path, process::Command};

use regex::Regex;
use serde_json::{Value, json};

use crate::{Result, cli::ReleaseSourceOptions, workspace::root_dir};

use super::support::{
    canonical_dir, commit_shape, read_json, sha256_file, string_field, write_json,
};

#[derive(Debug)]
pub(crate) struct RepositoryRef {
    pub(crate) repository: String,
    pub(crate) tag_object: String,
    pub(crate) commit: String,
}

pub(crate) fn validate(options: &ReleaseSourceOptions, frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    let tag = options.tag.as_str();
    let identity = release_identity(tag)?;
    repository_slug(options.backend_repository.as_str(), "backend")?;
    repository_slug(options.frontend_repository.as_str(), "frontend")?;
    commit_sha(options.backend_commit.as_str(), "backend")?;
    commit_sha(options.frontend_commit.as_str(), "frontend")?;
    let version = workspace_version(&root)?;
    if version != identity.1 {
        return Err(format!(
            "workspace version is {version:?}, tag requires {:?}",
            identity.1
        )
        .into());
    }
    annotated_tag_notes(&root, tag, "backend")?;
    validate_workspace_packages(&root, &identity.1)?;
    let mut backend =
        validate_repository_ref(&root, tag, options.backend_commit.as_str(), "backend")?;
    backend.repository = options.backend_repository.as_str().to_owned();
    let backend_openapi =
        validate_openapi(&root.join("openapi/openapi.json"), &identity.1, "backend")?;
    let frontend = canonical_dir(frontend_dir, "前端目录")?;
    validate_package_version(&frontend, &identity.1)?;
    let frontend_openapi = validate_openapi(
        &frontend.join("openapi/openapi.json"),
        &identity.1,
        "frontend",
    )?;
    annotated_tag_notes(&frontend, tag, "frontend")?;
    let mut frontend_ref =
        validate_repository_ref(&frontend, tag, options.frontend_commit.as_str(), "frontend")?;
    frontend_ref.repository = options.frontend_repository.as_str().to_owned();
    validate_frontend_contract_source(
        &frontend,
        options.backend_repository.as_str(),
        options.backend_commit.as_str(),
        &frontend_openapi,
    )?;
    let manifest = release_manifest(
        &identity,
        &backend,
        &frontend_ref,
        &backend_openapi,
        &frontend_openapi,
    )?;
    write_json(&options.manifest_path, &manifest)?;
    println!(
        "Release inputs are consistent: {tag}, backend/frontend/OpenAPI {}; manifest {}",
        identity.1,
        options.manifest_path.display()
    );
    Ok(())
}

pub(crate) fn release_identity(tag: &str) -> Result<(String, String)> {
    let pattern = Regex::new(r"^v(?P<version>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))$")
        .map_err(|error| format!("发布标签规则无效：{error}"))?;
    let captures = pattern.captures(tag).ok_or_else(|| {
        "release tag must be canonical vMAJOR.MINOR.PATCH \
             (prerelease tags are not supported)"
            .to_string()
    })?;
    let version = captures
        .name("version")
        .ok_or_else(|| "release tag version is missing".to_string())?
        .as_str();
    Ok((tag.to_owned(), version.to_owned()))
}

fn repository_slug(value: &str, label: &str) -> Result<()> {
    let pattern = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*$")
        .map_err(|error| format!("仓库标识规则无效：{error}"))?;
    if !pattern.is_match(value) {
        return Err(format!("{label} repository must be an owner/name slug, got {value:?}").into());
    }
    Ok(())
}

fn commit_sha(value: &str, label: &str) -> Result<()> {
    if !commit_shape(Some(value)) {
        return Err(
            format!("{label} commit must be a lowercase 40-character SHA, got {value:?}").into(),
        );
    }
    Ok(())
}

fn workspace_version(root: &Path) -> Result<String> {
    let path = root.join("Cargo.toml");
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read workspace manifest {}: {error}", path.display()))?;
    let manifest = toml::from_str::<toml::Value>(&source).map_err(|error| {
        format!(
            "cannot parse workspace manifest {}: {error}",
            path.display()
        )
    })?;
    manifest
        .get("workspace")
        .and_then(|value| value.get("package"))
        .and_then(|value| value.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "workspace.package.version is missing".to_string().into())
}

pub(crate) fn normalize_markdown(value: &str) -> String {
    let mut lines = value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|line| line.trim_end().to_owned())
        .collect::<Vec<_>>();
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

pub(crate) fn annotated_tag_notes(repository: &Path, tag: &str, label: &str) -> Result<String> {
    let reference = format!("refs/tags/{tag}");
    let object_type = git_text(repository, &["cat-file", "-t", &reference])?;
    if object_type != "tag" {
        return Err(
            format!("{label} tag {tag} must be an annotated tag, found {object_type}").into(),
        );
    }
    let notes = normalize_markdown(&git_text(
        repository,
        &["for-each-ref", "--format=%(contents)", &reference],
    )?);
    if notes.is_empty() {
        return Err(format!("{label} tag {tag} annotation must not be empty").into());
    }
    Ok(notes)
}

fn validate_workspace_packages(root: &Path, expected: &str) -> Result<()> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata output is invalid JSON: {error}"))?;
    let members = metadata
        .get("workspace_members")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata workspace_members is missing".to_string())?
        .iter()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let mismatches = metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata packages is missing".to_string())?
        .iter()
        .filter(|package| {
            package
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| members.contains(id))
                && package.get("version").and_then(Value::as_str) != Some(expected)
        })
        .map(|package| {
            format!(
                "{}={}",
                package.get("name").and_then(Value::as_str).unwrap_or("?"),
                package
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
            )
        })
        .collect::<Vec<_>>();
    if !mismatches.is_empty() {
        return Err(format!(
            "workspace packages must all be {expected}: {}",
            mismatches.join(", ")
        )
        .into());
    }
    Ok(())
}

pub(crate) fn validate_repository_ref(
    repository: &Path,
    tag: &str,
    expected_commit: &str,
    label: &str,
) -> Result<RepositoryRef> {
    let actual = git_text(repository, &["rev-parse", &format!("{tag}^{{commit}}")])?;
    if actual != expected_commit {
        return Err(format!(
            "{label} tag {tag} resolves to {actual}, expected fixed commit {expected_commit}"
        )
        .into());
    }
    Ok(RepositoryRef {
        repository: String::new(),
        tag_object: git_text(repository, &["rev-parse", &format!("refs/tags/{tag}")])?,
        commit: actual,
    })
}

pub(crate) fn validate_openapi(path: &Path, expected: &str, label: &str) -> Result<String> {
    let (contract, _) = read_json(path, &format!("{label} OpenAPI document"), 16 * 1024 * 1024)?;
    let info = contract
        .get("info")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{label} OpenAPI info must be an object"))?;
    let version = string_field(info, "version", &format!("{label} OpenAPI info")).unwrap_or("");
    if version != expected {
        return Err(
            format!("{label} OpenAPI info.version is {version:?}, expected {expected:?}").into(),
        );
    }
    sha256_file(path, &format!("{label} OpenAPI document"))
}

fn validate_package_version(frontend: &Path, expected: &str) -> Result<()> {
    let (package, _) = read_json(
        &frontend.join("package.json"),
        "frontend package.json",
        4 * 1024 * 1024,
    )?;
    let actual = package.get("version").and_then(Value::as_str).unwrap_or("");
    if actual != expected {
        return Err(
            format!("frontend package.json version is {actual:?}, expected {expected:?}").into(),
        );
    }
    Ok(())
}

pub(crate) fn validate_frontend_contract_source(
    frontend: &Path,
    backend_repository: &str,
    backend_commit: &str,
    openapi_hash: &str,
) -> Result<()> {
    let (source, _) = read_json(
        &frontend.join("openapi/source.json"),
        "frontend OpenAPI source",
        4 * 1024 * 1024,
    )?;
    let object = source
        .as_object()
        .ok_or_else(|| "frontend OpenAPI source must be an object".to_string())?;
    let expected = [
        ("schema_version", json!(1)),
        ("backend_repository", json!(backend_repository)),
        ("backend_commit", json!(backend_commit)),
        ("openapi_path", json!("openapi/openapi.json")),
        ("sha256", json!(openapi_hash)),
    ];
    for (field, value) in expected {
        if object.get(field) != Some(&value) {
            return Err(format!(
                "frontend openapi/source.json {field} is {:?}, expected {value:?}",
                object.get(field)
            )
            .into());
        }
    }
    Ok(())
}

pub(crate) fn release_manifest(
    identity: &(String, String),
    backend: &RepositoryRef,
    frontend: &RepositoryRef,
    backend_openapi: &str,
    frontend_openapi: &str,
) -> Result<Value> {
    if backend_openapi != frontend_openapi {
        return Err(format!(
            "backend and frontend OpenAPI hashes differ: {backend_openapi} != {frontend_openapi}"
        )
        .into());
    }
    Ok(json!({
        "backend": {
            "commit": backend.commit,
            "openapi_sha256": backend_openapi,
            "repository": backend.repository,
            "tag_object": backend.tag_object,
            "version": identity.1,
        },
        "contract": {"openapi_sha256": backend_openapi},
        "frontend": {
            "commit": frontend.commit,
            "openapi_sha256": frontend_openapi,
            "repository": frontend.repository,
            "tag_object": frontend.tag_object,
            "version": identity.1,
        },
        "release": {"tag": identity.0, "version": identity.1},
        "schema_version": 1,
    }))
}

fn git_text(repository: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| format!("cannot run git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| "git output is not UTF-8".into())
}
