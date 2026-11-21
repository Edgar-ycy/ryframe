//! 从正式预检收尾派生只可用于产品执行的审阅，保留原服务控制权。
use super::{
    SIDES, file_binding, path_text_lexical, portable_path, validate_future_root, validate_output,
    validate_review, write_new,
};
use crate::{
    Result,
    cli::FixtureReconciledReview,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{
            digest, json as document, resources::common, target_verify::fresh::reconcile,
        },
        fixture_control::{services::lifecycle::query, source_pair::execution},
    },
};
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) struct SeedService {
    pub(crate) run: PathBuf,
    pub(crate) review: PathBuf,
    pub(crate) request: Value,
}

struct Origin {
    review: Value,
    review_binding: Value,
    marker: Value,
    service: SeedService,
}

impl Origin {
    fn load(backend: &Path, workspace: &Path) -> Result<Self> {
        let workspace =
            validate_local_test_path(workspace, backend, LocalTestPathKind::ExistingDirectory)?;
        let origin = reconcile::seed_origin(backend, &workspace)?;
        let binding = &origin.request["review"];
        let review_binding = file_descriptor(binding);
        let review = common::read_bound(backend, &review_binding, "原 seed 审阅")?;
        validate_review(&review)?;
        if has_origin(&review) || binding["canonical_sha256"] != digest::plan_hash(&review)? {
            return Err("接续须使用原服务审阅及其正式收尾".into());
        }
        let manifest = common::read_bound(backend, &origin.storage_run["manifest"], "原服务账本")?;
        let run = PathBuf::from(document::text(&origin.storage_run["path"], "原服务目录")?);
        let expected = query::dispatch(backend, "service-run", &json!({"review":review}))?;
        if manifest["kind"] != "reference-fixture-service-run"
            || manifest["review"] != review_binding
            || expected != json!(run)
            || common::binding(&run.join("state.json"))? != origin.storage_run["state"]
            || review["scopes"]["seed"]["scope_id"] != origin.request["target"]["scope_id"]
        {
            return Err("收尾、原审阅和服务账本未绑定同一 seed".into());
        }
        let marker = json!({"workspace":workspace,"registration":origin.registration,
            "reconciliation":origin.reconciliation,"storage_run":origin.storage_run});
        let service = SeedService {
            run,
            review: PathBuf::from(document::text(&review_binding["path"], "原服务审阅路径")?),
            request: origin.request,
        };
        Ok(Self {
            review,
            review_binding,
            marker,
            service,
        })
    }
}

pub(crate) fn has_origin(review: &Value) -> bool {
    review["sources"].get("reconciled_seed").is_some()
}

pub(crate) fn service_run(review: &Value) -> Result<Option<PathBuf>> {
    if !has_origin(review) {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(document::text(
        &review["sources"]["reconciled_seed"]["storage_run"]["path"],
        "接续服务目录",
    )?)))
}

fn file_descriptor(binding: &Value) -> Value {
    json!({"path":binding["path"],"bytes":binding["bytes"],"sha256":binding["sha256"]})
}

/// 在签发和消费两端核对同一服务运行、同一目标和原凭据字节。
pub(crate) fn validate_request(backend: &Path, request: &Value, run: &Path) -> Result<()> {
    let review = common::read_bound(backend, &file_descriptor(&request["review"]), "fresh 审阅")?;
    let Some(origin) = verify(backend, &review)? else {
        return Ok(());
    };
    if origin.run != run {
        return Err("接续 seed 请求必须登记原服务运行".into());
    }
    verify_request_projection(&origin.request, request)?;
    if redis_configuration(request)? != redis_configuration(&origin.request)? {
        return Err("接续 seed 请求改变了已登记的 Redis 配置".into());
    }
    Ok(())
}

fn verify_request_projection(previous: &Value, request: &Value) -> Result<()> {
    if request["side"] != "seed"
        || request["id"] == previous["id"]
        || request["maintenance_build"] == previous["maintenance_build"]
        || request["target"]["scope_id"] != previous["target"]["scope_id"]
        || request["target"]["s3"] != previous["target"]["s3"]
        || request["storage"] != previous["storage"]
        || request["reset"]["credential_version"] != previous["reset"]["credential_version"]
        || request["reset"]["sentinel_key"] != previous["reset"]["sentinel_key"]
    {
        return Err("接续 seed 请求改变了原服务、对象配置或资源范围".into());
    }
    let old_databases = document::array(&previous["target"]["databases"], "原四库")?;
    let new_databases = document::array(&request["target"]["databases"], "接续四库")?;
    if new_databases.len() != 4 || old_databases.len() != 4 {
        return Err("接续 seed 请求必须精确绑定原四库".into());
    }
    for previous in old_databases {
        let mut matches = new_databases
            .iter()
            .filter(|item| item["key"] == previous["key"]);
        let database = matches.next().ok_or("接续 seed 请求改变了数据库键")?;
        let mut original = previous.clone();
        original["defaults_file"] = database["defaults_file"].clone();
        if matches.next().is_some() || original != *database {
            return Err("接续 seed 请求改变了数据库身份或客户端文件字节".into());
        }
    }
    Ok(())
}

fn redis_configuration(request: &Value) -> Result<toml::Value> {
    let root = Path::new(document::text(
        &request["execution_backend"]["path"],
        "seed 执行根",
    )?);
    let config = Path::new(document::text(&request["config_dir"], "seed 配置目录")?);
    let table = crate::rate_limit::load_app_table(root, Some(config), "test")
        .map_err(|_| "无法核验 seed 的 Redis 配置")?;
    table
        .get("redis")
        .cloned()
        .ok_or_else(|| "seed 配置缺少 Redis".into())
}

/// 重新派生产品审阅，只准移动执行、运行及配置文件路径。
pub(crate) fn verify(backend: &Path, review: &Value) -> Result<Option<SeedService>> {
    if !has_origin(review) {
        return Ok(None);
    }
    let marker = &review["sources"]["reconciled_seed"];
    let workspace = Path::new(document::text(&marker["workspace"], "原已收尾工作区")?);
    let origin = Origin::load(backend, workspace)?;
    let publication = common::read_bound(backend, &review["execution_source"], "接续执行源码发布")?;
    if *review != derive(backend, &origin, review, &publication)? {
        return Err("产品接续审阅改变了原服务或已登记资源".into());
    }
    Ok(Some(origin.service))
}

pub(super) fn publish(backend: &Path, options: &FixtureReconciledReview) -> Result<Value> {
    let origin = Origin::load(backend, &options.workspace)?;
    let binding = execution_source_binding(backend, &options.execution_source)?;
    let publication = execution::verify(backend, &binding)?;
    let execution = execution_root(&publication)?;
    validate_future_root(&options.future_root, &execution)?;
    let proposed = json!({"execution_source":binding,"future_root":path_text_lexical(&options.future_root)?,
        "created_at":Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false)});
    let review = derive(backend, &origin, &proposed, &publication)?;
    validate_review(&review)?;
    verify(backend, &review)?;
    validate_future_root(&options.future_root, &execution)?;
    validate_output(&options.output, backend)?;
    write_new(&options.output, &review)?;
    if super::read_controlled_json(&options.output, "产品接续审阅")? != review {
        return Err("产品接续审阅发布后发生变化".into());
    }
    Ok(json!({"status":"reconciled_seed_reviewed","services_started":false,"remote_writes":0}))
}

fn execution_source_binding(backend: &Path, file: &Path) -> Result<Value> {
    let file = validate_local_test_path(file, backend, LocalTestPathKind::ExistingFile)?;
    let file = portable_path(&std::fs::canonicalize(file)?);
    file_binding(&file, "接续执行源码发布")
}

fn execution_root(publication: &Value) -> Result<PathBuf> {
    Ok(PathBuf::from(document::text(
        &publication["published"]["backend"]["path"],
        "接续产品执行根",
    )?))
}

fn derive(backend: &Path, origin: &Origin, proposed: &Value, publication: &Value) -> Result<Value> {
    let execution = execution_root(publication)?;
    validate_local_test_path(&execution, backend, LocalTestPathKind::ExistingDirectory)?;
    let future = portable_path(Path::new(document::text(
        &proposed["future_root"],
        "接续运行目录",
    )?));
    validate_local_test_path(&future, backend, LocalTestPathKind::StateDirectory)?;
    if future.parent() != Some(execution.join(".local-tests/reference-fixture").as_path())
        || proposed["execution_source"] == origin.review["execution_source"]
        || json!(execution) == origin.review["scopes"]["seed"]["backend_dir"]
    {
        return Err("产品接续须绑定新的独立发布及其参考夹具运行目录".into());
    }
    let sources = &publication["original"]["source_pair"]["sources"];
    let backend_head = document::text(&sources["backend"]["head"], "接续后端提交")?;
    let frontend_head = document::text(&sources["frontend"]["head"], "接续前端提交")?;
    let created = document::text(&proposed["created_at"], "接续审阅时间")?;
    chrono::DateTime::parse_from_rfc3339(created)?;
    let mut review = origin.review.clone();
    review["created_at"] = json!(created);
    review["future_root"] = json!(path_text_lexical(&future)?);
    review["execution_source"] = proposed["execution_source"].clone();
    review["sources"] = json!({"backend_head":backend_head,"frontend_head":frontend_head,
        "renewed_from":origin.review_binding,"reconciled_seed":origin.marker});
    let connection = path_text_lexical(
        &execution.join(".local-tests/reference-fixture/secrets/mysql-client.cnf"),
    )?;
    for side in SIDES {
        let scope = &mut review["scopes"][*side];
        scope["backend_dir"] = json!(path_text_lexical(&execution)?);
        scope["runtime_dir"] = json!(path_text_lexical(&future.join(format!("runtime-{side}")))?);
        scope["identity_ledger"] = json!(path_text_lexical(
            &future.join(format!("identities-{side}"))
        )?);
        for db in scope["databases"].as_array_mut().ok_or("原审阅缺少四库")? {
            db["connection_file"] = json!(connection);
        }
    }
    Ok(review)
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/fixture_control/reconciled_review.rs"]
mod tests;
