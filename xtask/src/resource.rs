use std::path::Path;

use crate::{Result, cli::ResourceCommand};

#[cfg(feature = "resource")]
use std::fs;

#[cfg(feature = "resource")]
use crate::{
    cli::{ApiSyncCommand, ResourceAction},
    workspace::root_dir,
};

#[cfg(feature = "resource")]
use ryframe_generator::{
    GeneratedCatalog, PlanAction, ResourceError, ResourceIr, ResourceWorkspace, load_resource,
    plan_resource_changes, render_resources, write_resource,
};

#[cfg(feature = "resource")]
pub(crate) fn run(command: &ResourceCommand, frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    let resources = load_catalog(&root)?;
    if !resources
        .iter()
        .any(|resource| resource.name == command.name)
    {
        return Err(ResourceError::new(
            format!("资源 `{}` 不存在", command.name),
            format!(
                "在 catalog/resources/{}.toml 中建立清单，或检查清单内 resource.name",
                command.name
            ),
        )
        .with_resource(&command.name)
        .into());
    }
    let catalog = render_resources(&resources)?;

    match command.action {
        ResourceAction::Preview => preview(&catalog, &command.name, &root, frontend_dir),
        ResourceAction::Explain => explain(&catalog, &command.name),
        ResourceAction::Write => write(&catalog, &command.name, &root, frontend_dir),
    }
}

#[cfg(not(feature = "resource"))]
pub(crate) fn run(command: &ResourceCommand, _frontend_dir: &Path) -> Result<()> {
    Err(format!(
        "内部调用缺少 resource feature，未修改文件；请使用 `cargo resource {}{}`",
        command.name,
        match command.action {
            crate::cli::ResourceAction::Preview => "",
            crate::cli::ResourceAction::Write => " --write",
            crate::cli::ResourceAction::Explain => " --explain",
        }
    )
    .into())
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
            "共 {changed} 个文件需要新增、更新或删除；确认后运行 `cargo resource {resource} --write`。"
        );
    }
    Ok(())
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
    if let Err(error) = crate::contract::api_sync(&ApiSyncCommand::Candidate, frontend_root) {
        return Err(format!(
            "资源代码已经安全写入，但候选 OpenAPI 刷新失败：{error}；修复编译或契约错误后运行 `cargo api-sync`"
        )
        .into());
    }
    println!("候选 OpenAPI 与前端派生契约已刷新。");
    Ok(())
}
