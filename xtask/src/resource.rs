use std::path::Path;

use crate::{
    Result,
    cli::{ResourceCommand, ResourceTarget},
};

#[cfg(feature = "resource")]
use std::fs;

#[cfg(feature = "resource")]
use crate::{
    cli::{ApiGenerateCommand, ResourceAction},
    workspace::root_dir,
};

#[cfg(feature = "resource")]
use ryframe_generator::{
    GeneratedCatalog, PlanAction, ResourceAssetPlan, ResourceError, ResourceIr, ResourceWorkspace,
    load_resource, plan_all_resource_changes, plan_resource_changes, render_resources,
    write_resource, write_resources,
};

#[cfg(feature = "resource")]
pub(crate) fn run(command: &ResourceCommand, frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    let resources = load_catalog(&root)?;
    if let ResourceTarget::Named(name) = &command.target
        && !resources.iter().any(|resource| resource.name == *name)
    {
        return Err(missing_resource(name).into());
    }
    let catalog = render_resources(&resources)?;

    match (&command.target, command.action) {
        (ResourceTarget::Named(name), ResourceAction::Preview) => {
            preview(&catalog, name, &root, frontend_dir)
        }
        (ResourceTarget::Named(name), ResourceAction::Check) => {
            check_named(&catalog, name, &root, frontend_dir)
        }
        (ResourceTarget::Named(name), ResourceAction::Explain) => explain(&catalog, name),
        (ResourceTarget::Named(name), ResourceAction::Write) => {
            write(&catalog, name, &root, frontend_dir)
        }
        (ResourceTarget::All, ResourceAction::Check) => check_all(&catalog, &root, frontend_dir),
        (ResourceTarget::All, ResourceAction::Write) => write_all(&catalog, &root, frontend_dir),
        (ResourceTarget::All, _) => {
            Err("`--all` 只支持 `cargo xtask generate resource --all --check|--write`".into())
        }
    }
}

#[cfg(not(feature = "resource"))]
pub(crate) fn run(command: &ResourceCommand, frontend_dir: &Path) -> Result<()> {
    let mut arguments = vec![
        "run".to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        "target/xtask-resource".to_owned(),
        "-p".to_owned(),
        "xtask".to_owned(),
        "--features".to_owned(),
        "resource".to_owned(),
        "--".to_owned(),
        "generate".to_owned(),
        "resource".to_owned(),
    ];
    arguments.extend(resource_arguments(command));
    arguments.push("--frontend-dir".to_owned());
    arguments.push(frontend_dir.to_string_lossy().into_owned());
    crate::process::run_owned(&crate::workspace::root_dir(), "cargo", &arguments)
}

#[cfg(not(feature = "resource"))]
fn resource_arguments(command: &ResourceCommand) -> Vec<String> {
    let mut arguments = vec![match &command.target {
        ResourceTarget::Named(name) => name.clone(),
        ResourceTarget::All => "--all".to_owned(),
    }];
    let action = match command.action {
        crate::cli::ResourceAction::Preview => None,
        crate::cli::ResourceAction::Check => Some("--check"),
        crate::cli::ResourceAction::Write => Some("--write"),
        crate::cli::ResourceAction::Explain => Some("--explain"),
    };
    arguments.extend(action.map(str::to_owned));
    arguments
}

#[cfg(feature = "resource")]
fn missing_resource(name: &str) -> ResourceError {
    ResourceError::new(
        format!("资源 `{name}` 不存在"),
        format!("在 catalog/resources/{name}.toml 中建立清单，或检查清单内 resource.name"),
    )
    .with_resource(name)
}

#[cfg(feature = "resource")]
pub(crate) fn load_catalog(root: &Path) -> Result<Vec<ResourceIr>> {
    let directory = root.join("catalog/resources");
    let mut paths = fs::read_dir(&directory)
        .map_err(|error| {
            ResourceError::file(
                &directory,
                format!("无法读取资源清单目录：{error}"),
                "创建 catalog/resources 并加入至少一个资源 TOML",
            )
        })?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| is_resource_manifest(path));
    paths.sort();
    if paths.is_empty() {
        return Err(ResourceError::file(
            &directory,
            "资源清单目录中没有 TOML",
            "新增 catalog/resources/<资源名>.toml",
        )
        .into());
    }
    paths
        .into_iter()
        .map(load_resource)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

#[cfg(feature = "resource")]
fn is_resource_manifest(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|extension| extension == "toml")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !name.starts_with('.'))
}

#[cfg(feature = "resource")]
fn preview(
    catalog: &GeneratedCatalog,
    resource: &str,
    backend_root: &Path,
    frontend_root: &Path,
) -> Result<()> {
    println!("资源 `{resource}` 的生成计划：");
    let workspace = ResourceWorkspace {
        backend_root,
        frontend_root: Some(frontend_root),
    };
    let mut selected = plan_resource_changes(catalog, resource, workspace)?.assets;
    selected.sort_by(|left, right| {
        left.root
            .cmp(&right.root)
            .then_with(|| left.path.as_str().cmp(right.path.as_str()))
    });
    let mut changed = 0_usize;
    for asset in &selected {
        let state = match asset.action {
            PlanAction::Create => "新增",
            PlanAction::Update => "变更",
            PlanAction::Delete => "删除",
            PlanAction::Unchanged => {
                println!("  一致 {}:{}", asset.root.label(), asset.path);
                continue;
            }
        };
        println!("  {state} {}:{}", asset.root.label(), asset.path);
        print!(
            "{}",
            crate::diff::unified(&asset.before, &asset.after, &asset.path)
        );
        changed += 1;
    }
    if changed == 0 {
        println!("生成结果与工作区一致，无需写入。");
    } else {
        println!(
            "共 {changed} 个文件需要新增、更新或删除；确认后运行 `cargo xtask generate resource {resource} --write`。"
        );
    }
    Ok(())
}

#[cfg(feature = "resource")]
fn check_named(
    catalog: &GeneratedCatalog,
    resource: &str,
    backend_root: &Path,
    frontend_root: &Path,
) -> Result<()> {
    let plan = plan_resource_changes(
        catalog,
        resource,
        ResourceWorkspace {
            backend_root,
            frontend_root: Some(frontend_root),
        },
    )?;
    ensure_clean(
        plan,
        &format!("资源 `{resource}`"),
        &format!(
            "运行 `cargo xtask generate resource {resource}` 查看差异，确认后显式执行 `--write`"
        ),
    )
}

#[cfg(feature = "resource")]
fn check_all(catalog: &GeneratedCatalog, backend_root: &Path, frontend_root: &Path) -> Result<()> {
    let plan = plan_all_resource_changes(
        catalog,
        ResourceWorkspace {
            backend_root,
            frontend_root: Some(frontend_root),
        },
    )?;
    ensure_clean(
        plan,
        "全部资源",
        "逐个运行 `cargo xtask generate resource <资源名>` 查看差异，确认后显式执行对应的 `--write`",
    )
}

#[cfg(feature = "resource")]
pub(crate) fn ensure_clean(plan: ResourceAssetPlan, scope: &str, suggestion: &str) -> Result<()> {
    let changed = plan
        .assets
        .iter()
        .filter(|asset| asset.action != PlanAction::Unchanged)
        .collect::<Vec<_>>();
    if changed.is_empty() {
        println!("{scope}：生成结果与工作区一致。");
        return Ok(());
    }
    eprintln!("{scope}：存在 {} 个待生成差异：", changed.len());
    for asset in &changed {
        let state = match asset.action {
            PlanAction::Create => "新增",
            PlanAction::Update => "变更",
            PlanAction::Delete => "删除",
            PlanAction::Unchanged => unreachable!("已过滤未变化资产"),
        };
        eprintln!("  {state} {}:{}", asset.root.label(), asset.path);
        eprint!(
            "{}",
            crate::diff::unified(&asset.before, &asset.after, &asset.path)
        );
    }
    Err(ResourceError::new(format!("{scope}：生成结果与工作区不一致"), suggestion).into())
}

#[cfg(feature = "resource")]
fn explain(catalog: &GeneratedCatalog, resource: &str) -> Result<()> {
    let explanation = catalog.explanation(resource).ok_or_else(|| {
        ResourceError::new(
            format!("资源 `{resource}` 缺少解释信息"),
            "重新运行生成器检查资源清单",
        )
        .with_resource(resource)
    })?;
    print!("{}", explanation.render_text());
    Ok(())
}

#[cfg(feature = "resource")]
fn write(
    catalog: &GeneratedCatalog,
    resource: &str,
    backend_root: &Path,
    frontend_root: &Path,
) -> Result<()> {
    let report = write_resource(
        catalog,
        resource,
        ResourceWorkspace {
            backend_root,
            frontend_root: Some(frontend_root),
        },
    )?;
    println!(
        "资源生成完成：写入 {}，删除 {}，未变化 {}。",
        report.written.len(),
        report.removed.len(),
        report.unchanged.len()
    );
    for path in &report.written {
        println!("  写入 {path}");
    }
    for path in &report.removed {
        println!("  删除 {path}");
    }
    refresh_api(frontend_root)
}

#[cfg(feature = "resource")]
fn write_all(catalog: &GeneratedCatalog, backend_root: &Path, frontend_root: &Path) -> Result<()> {
    let report = write_resources(
        catalog,
        ResourceWorkspace {
            backend_root,
            frontend_root: Some(frontend_root),
        },
    )?;
    println!(
        "全部资源生成完成：写入 {}，删除 {}，未变化 {}。",
        report.written.len(),
        report.removed.len(),
        report.unchanged.len()
    );
    refresh_api(frontend_root)
}

#[cfg(feature = "resource")]
fn refresh_api(frontend_root: &Path) -> Result<()> {
    let api = ApiGenerateCommand {
        reference: None,
        write: true,
    };
    if let Err(error) = crate::contract::generate_api(&api, frontend_root) {
        return Err(format!(
            "资源代码已经安全写入，但候选 OpenAPI 刷新失败：{error}；修复编译或契约错误后运行 `cargo xtask generate api --write`"
        )
        .into());
    }
    println!("候选 OpenAPI 与前端派生契约已刷新。");
    Ok(())
}
