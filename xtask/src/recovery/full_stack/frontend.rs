use std::{fs, path::Path};

use serde_json::json;

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::{
        clone::native::json as documents,
        fixture_control::{file_binding, portable_path, source_pair::execution::publish_frontend},
        frontend_build,
    },
};

use super::{Port, context::Context, provenance};

pub(super) fn run(context: &Context, port: &mut impl Port) -> Result<()> {
    validate_absolute_path(&context.output, LocalTestPathKind::ExistingDirectory)?;
    let output = portable_path(&fs::canonicalize(&context.output)?);
    let original = provenance::current(&context.root, &output)?;
    if original["fixture"] != "business"
        || original["source_pair"]["run_id"].as_u64() != context.run.parse().ok()
        || original["source_pair"]["attempt"].as_u64() != context.attempt.parse().ok()
        || original["source_pair"]["run_id"].is_null()
    {
        return Err("前端派生构建需要当前 CI 的业务夹具及原始双仓来源".into());
    }
    let fixture_frontend = Path::new(documents::text(
        &original["roots"]["frontend"],
        "业务夹具前端",
    )?);
    let backend = portable_path(&fs::canonicalize(&context.root)?);
    let destination = backend.join(".local-tests/full-stack-frontend");
    validate_local_test_path(&destination, &backend, LocalTestPathKind::NewDirectory)?;
    let evidence_path = output.join("frontend-source.json");
    let receipt_path = output.join("frontend-build.json");
    let install_log = output.join("frontend-install.log");
    let build_log = output.join("frontend-build.log");
    for path in [&evidence_path, &receipt_path, &install_log, &build_log] {
        validate_absolute_path(path, LocalTestPathKind::OutputFile)?;
    }

    let generated = &original["generated"]["frontend"];
    let published = publish_frontend(fixture_frontend, &destination, generated)?;
    pnpm(
        port,
        &destination,
        &["install", "--frozen-lockfile"],
        &install_log,
    )?;
    published.verify()?;
    pnpm(
        port,
        &destination,
        &["build", "--real", "--source-commit", published.head()],
        &build_log,
    )?;
    let source = published.verify()?;
    let proof = frontend_build::validate(
        &destination,
        Some(&source["inventory"]),
        Some(published.head()),
    )?;
    frontend_build::validate_ci_receipt(&proof.receipt, &source["inventory"], published.head())?;
    if provenance::current(&context.root, &output)? != original {
        return Err("构建期间本次全栈的原始源码组合或业务夹具发生变化".into());
    }
    documents::publish_new(&receipt_path, &proof.receipt, "前端产物收据")?;
    proof.verify()?;
    documents::publish_new(
        &evidence_path,
        &json!({
            "format_version": 1,
            "kind": "generated-business-frontend-build",
            "source_pair": original["source_pair"]["receipt"],
            "fixture": original["fixture_receipt"],
            "generated": generated,
            "published": source,
            "build": file_binding(&receipt_path, "归档前端构建收据")?,
        }),
        "业务前端来源收据",
    )?;
    println!("已构建并核验业务夹具前端派生提交 {}", published.head());
    Ok(())
}

fn pnpm(port: &mut impl Port, root: &Path, args: &[&str], log: &Path) -> Result<()> {
    let arguments = std::iter::once("pnpm")
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    port.command_at(root, "corepack", &arguments, Some(log))?;
    Ok(())
}
