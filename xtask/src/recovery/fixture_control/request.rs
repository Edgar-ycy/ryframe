//! 从固定产品环境和已登记服务代次签发 fresh-target 请求。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::{FixtureRequestCommand, FixtureRequestOptions},
    local_test_path::{LocalTestPathKind, same_local_test_file, validate_local_test_path},
    recovery::clone::native::{
        digest, json as document,
        resources::common,
        target_verify::{
            binding_request,
            input::{request, request_review},
        },
    },
};

use super::{
    artifact::runtime::configuration_digest, file_binding, output_value, portable_path,
    review::reconciled, services::lifecycle::query,
};

pub(super) fn run(backend: &Path, command: &FixtureRequestCommand) -> Result<Value> {
    let FixtureRequestCommand::Publish(options) = command else {
        return Err("request 帮助不能签发请求".into());
    };
    let backend = portable_path(&backend.canonicalize()?);
    let output = &options.output.output;
    output_value(output, &backend, "fresh-target 请求输出")?;
    let request = publish(output, || build(&backend, options))?;
    Ok(
        json!({"status":"requested","request_sha256":digest::plan_hash(&request)?,"remote_writes":0}),
    )
}

fn publish(output: &Path, mut build: impl FnMut() -> Result<Value>) -> Result<Value> {
    let request = build()?;
    document::write_new(output, &request)?;
    if document::read(output)? != request
        || build()? != request
        || document::read(output)? != request
    {
        return Err("fresh-target 请求发布期间输入发生变化；保留原输出，禁止重放".into());
    }
    Ok(request)
}

fn build(backend: &Path, options: &FixtureRequestOptions) -> Result<Value> {
    let side = options.side.as_str();
    let product = Product::load(backend, &options.environment, side)?;
    let run = common::local_directory(backend, &options.service_run, "fresh 服务账本")?;
    let service = load_service(backend, &run, &product.prepared, side)?;
    let value = product.request(&options.id, side, &service.storage)?;
    binding_request::validate(backend, &value, &Value::Null)?;
    let execution = request::execution_backend(
        backend,
        value.get("execution_backend"),
        &request::SystemSourceObserver,
    )?;
    product.check()?;
    service.observe(backend)?;
    if request::execution_backend(
        backend,
        value.get("execution_backend"),
        &request::SystemSourceObserver,
    )? != execution
    {
        return Err("fresh-target 执行来源在请求构造期间变化".into());
    }
    product.check()?;
    reconciled::validate_request(backend, &value, &run)?;
    Ok(value)
}

fn load_service(backend: &Path, run: &Path, prepared: &Prepared, side: &str) -> Result<Service> {
    if let Some(origin) = reconciled::verify(backend, &prepared.review)? {
        if side != "seed" || run != origin.run {
            return Err("接续请求只允许使用原 seed 服务账本".into());
        }
        return Service::load(backend, run, &origin.review);
    }
    let expected = query::dispatch(backend, "service-run", &json!({"review":prepared.review}))?;
    if run != Path::new(document::text(&expected, "审阅服务账本")?) {
        return Err("fresh-target 请求必须绑定本审阅计划的唯一服务账本".into());
    }
    let service = Service::load(backend, run, &prepared.review_path)?;
    if side == "seed" {
        require_seed_bootstrap(
            backend,
            &prepared.binding,
            &service.context["sources"]["bootstrap"],
        )?;
    }
    Ok(service)
}

fn require_seed_bootstrap(backend: &Path, selected: &Value, recorded: &Value) -> Result<()> {
    let recorded_path = common::bound_file(backend, recorded, "原 seed 私有环境")?;
    let selected_path = Path::new(document::text(&selected["path"], "seed 私有环境路径")?);
    if !same_local_test_file(backend, selected_path, &recorded_path)?
        || selected["bytes"] != recorded["bytes"]
        || selected["sha256"] != recorded["sha256"]
    {
        return Err("seed 请求必须使用启动首代服务的同一私有环境".into());
    }
    Ok(())
}

struct Prepared {
    path: PathBuf,
    binding: Value,
    receipt: Value,
    review_path: PathBuf,
    review: Value,
}

impl Prepared {
    fn load(backend: &Path, path: &Path, side: &str) -> Result<Self> {
        let path = validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)?;
        let binding = file_binding(&path, "fresh 私有环境")?;
        let receipt = document::read(&path)?;
        if receipt["kind"] != "reference-fixture-environment"
            || receipt["status"] != "prepared"
            || receipt["services_started"] != false
            || receipt["remote_writes"] != 0
        {
            return Err("私有环境收据无效或已包含服务副作用".into());
        }
        let plan = &receipt["plan"];
        let mut body = document::object(plan, "私有环境计划")?.clone();
        let hash = body.remove("sha256").ok_or("私有环境计划缺少摘要")?;
        if plan["kind"] != "reference-fixture-environment-plan"
            || plan["side"] != side
            || hash != digest::plan_hash(&Value::Object(body))?
        {
            return Err("私有环境侧或计划摘要不同".into());
        }
        let descriptor = &plan["review"];
        let review = request_review::read(backend, descriptor)?;
        let review_path = PathBuf::from(document::text(&descriptor["path"], "私有环境审阅")?);
        let selected = &review["scopes"][side];
        let execution = Path::new(document::text(&receipt["execution_backend"], "执行工作树")?);
        if plan["scope_id"] != selected["scope_id"]
            || execution != Path::new(document::text(&selected["backend_dir"], "审阅执行工作树")?)
        {
            return Err("私有环境执行工作树与指定审阅侧不符".into());
        }
        let prepared = Self {
            path,
            binding,
            receipt,
            review_path,
            review,
        };
        prepared.check()?;
        Ok(prepared)
    }

    fn check(&self) -> Result<()> {
        let plan = &self.receipt["plan"]["review"];
        let review = file_binding(&self.review_path, "fresh 审阅计划")?;
        if file_binding(&self.path, "fresh 私有环境")? != self.binding
            || review != json!({"path":plan["path"],"bytes":plan["bytes"],"sha256":plan["sha256"]})
        {
            return Err("fresh-target 私有环境或审阅计划在请求构造期间变化".into());
        }
        Ok(())
    }
}

struct Product {
    prepared: Prepared,
    execution: PathBuf,
    environment: PathBuf,
    environment_binding: Value,
    configuration: PathBuf,
    configuration_sha256: String,
    databases: Vec<Value>,
    settings: Value,
}

impl Product {
    fn load(backend: &Path, path: &Path, side: &str) -> Result<Self> {
        let prepared = Prepared::load(backend, path, side)?;
        let execution = PathBuf::from(document::text(
            &prepared.receipt["execution_backend"],
            "执行工作树",
        )?);
        let environment = prepared
            .path
            .parent()
            .ok_or("私有环境无父目录")?
            .join("environment.json");
        let environment =
            validate_local_test_path(&environment, backend, LocalTestPathKind::ExistingFile)?;
        let environment_binding = file_binding(&environment, "fresh 私有环境变量")?;
        let values = document::read(&environment)?;
        let configuration =
            PathBuf::from(document::text(&prepared.receipt["config_dir"], "配置目录")?);
        request::config_directory(
            backend,
            &json!({"config_dir":configuration,
            "execution_backend":{"path":execution}}),
        )?;
        let configuration_sha256 = configuration_digest(&configuration)?;
        let settings = serde_json::to_value(
            crate::rate_limit::load_app_table(&execution, Some(&configuration), "test")
                .map_err(|_| "fresh-target 无法读取绑定的测试配置")?,
        )?;
        let selected = &prepared.review["scopes"][side];
        if values["environment"]["APP_ENV"] != "test"
            || settings["scope_id"] != selected["scope_id"]
            || prepared.receipt["configuration_sha256"] != configuration_sha256
        {
            return Err("fresh-target 私有环境未绑定当前 scope 或配置摘要".into());
        }
        for key in ["access_key", "secret_key"] {
            document::text(&settings["object_storage"][key], "对象存储须显式配置认证")?;
        }
        let databases = database_bindings(backend, selected)?;
        let product = Self {
            prepared,
            execution,
            environment,
            environment_binding,
            configuration,
            configuration_sha256,
            databases,
            settings,
        };
        product.check()?;
        Ok(product)
    }

    fn check(&self) -> Result<()> {
        self.prepared.check()?;
        if file_binding(&self.environment, "fresh 私有环境变量")? != self.environment_binding
            || configuration_digest(&self.configuration)? != self.configuration_sha256
        {
            return Err("fresh-target 私有环境或配置在请求构造期间变化".into());
        }
        for database in &self.databases {
            let path = Path::new(document::text(
                &database["defaults_file"],
                "MySQL 凭据文件",
            )?);
            if digest::file(path)?.sha256 != database["defaults_sha256"] {
                return Err("fresh-target MySQL 凭据在请求构造期间变化".into());
            }
        }
        Ok(())
    }

    fn request(&self, id: &str, side: &str, storage: &Value) -> Result<Value> {
        let review = &self.prepared.review;
        let plan = &self.prepared.receipt["plan"];
        let selected = &review["scopes"][side];
        let scope = document::text(&selected["scope_id"], "目标 scope")?;
        let tool = &review["tools"]["rustfs"];
        if storage["rustfs"]["identity"]["executable"] != tool["path"]
            || storage["rustfs"]["sha256"] != tool["sha256"]
        {
            return Err("RustFS 当前登记二进制与审阅工具不符".into());
        }
        let credentials = &self.settings["object_storage"];
        let mysql = &review["tools"]["mysql"];
        Ok(json!({
            "format_version":1, "kind":"devex-clone-fresh-target", "id":id,
            "review":plan["review"], "side":side,
            "target":{"scope_id":scope,"s3":{"endpoint":selected["objects"]["endpoint"],
                "region":selected["objects"]["region"],"access_key":credentials["access_key"],
                "secret_key":credentials["secret_key"]},"databases":self.databases},
            "maintenance_build":plan["maintenance_build"],
            "configuration_sha256":self.configuration_sha256,"config_dir":self.configuration,
            "tools":{"mysql":{"path":mysql["path"],"sha256":mysql["sha256"]}},
            "storage":storage,
            "reset":{"legacy_ownership":{"mysql_exclusive":true,"redis_exclusive":true,
                "object_storage_exclusive":true},
                "credential_version":self.settings["reset"]["credential_version"],
                "sentinel_key":format!("ryframe:devex-fresh:{scope}:sentinel"),
                "sentinel_value":format!("devex-fresh:{id}")},
            "execution_backend":{"execution_source":plan["execution_source"],"path":self.execution},
        }))
    }
}

fn database_bindings(backend: &Path, selected: &Value) -> Result<Vec<Value>> {
    let databases = document::array(&selected["databases"], "审阅数据库")?;
    request::KEYS
        .iter()
        .map(|(key, kind, mode)| {
            let item = databases
                .iter()
                .find(|item| item["key"] == *key)
                .ok_or("审阅数据库缺失")?;
            let path = Path::new(document::text(&item["connection_file"], "MySQL 凭据路径")?);
            validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)?;
            Ok(
                json!({"key":key,"kind":kind,"mode":mode,"database":item["database"],
            "server_uuid":item["expected_server_uuid"],"defaults_file":path,
            "defaults_sha256":digest::file(path)?.sha256}),
            )
        })
        .collect()
}

struct Service {
    context: Value,
    registered: Value,
    storage: Value,
}

impl Service {
    fn load(backend: &Path, run: &Path, review: &Path) -> Result<Self> {
        let manifest = file_binding(&run.join("manifest.json"), "服务清单")?;
        let value = common::read_bound(backend, &manifest, "服务清单")?;
        let seed = common::bound_file(backend, &value["bootstrap"], "原 seed 私有环境")?;
        let seed = Prepared::load(backend, &seed, "seed")?;
        if seed.binding != value["bootstrap"]
            || file_binding(&seed.review_path, "原服务审阅")? != value["review"]
        {
            return Err("首代服务账本未绑定原 seed 私有环境和审阅".into());
        }
        let context = query::dispatch(
            backend,
            "service-context",
            &json!({"review":review,"environment":seed.path}),
        )?;
        if Path::new(document::text(&context["run"], "服务账本路径")?) != run
            || context["sources"]["manifest"] != manifest
            || context["sources"]["bootstrap"] != seed.binding
            || context["history"]["closed"] != false
            || !context["history"]["external_recovery"].is_null()
        {
            return Err("首代服务账本发生变化、已关闭或外部终止".into());
        }
        let registered = query::dispatch(
            backend,
            "service-registered",
            &json!({"value":context,"settled":true}),
        )?;
        let rustfs = &registered["storage"];
        let storage = json!({"rustfs":{"identity":rustfs["identity"],"sha256":rustfs["sha256"],
            "process_receipt":rustfs["process_receipt"],"launch_receipt":rustfs["launch_receipt"]},
            "redis":registered["runtime"]["redis"]});
        if context["history"]["active_generation"]["kind"] != "initial" {
            let transition = query::dispatch(
                backend,
                "service-transition",
                &json!({"run":run,"expected":registered["origin"]}),
            )?;
            let pair = document::array(&transition, "已登记服务后继")?;
            if pair.len() != 2
                || pair[0]["storage"] != storage["rustfs"]
                || pair[1]["redis"] != storage["redis"]
            {
                return Err("fresh-target 当前服务投影不属于已登记的重启代次".into());
            }
        }
        Ok(Self {
            context,
            registered,
            storage,
        })
    }

    fn observe(&self, backend: &Path) -> Result<()> {
        self.observe_with(backend, |registered| {
            query::dispatch(backend, "service-observe", &json!({"services":registered}))
        })
    }

    fn observe_with(
        &self,
        backend: &Path,
        observe: impl FnOnce(&Value) -> Result<Value>,
    ) -> Result<()> {
        let observed = observe(&self.registered)?;
        if observed != json!({"redis":"running","rustfs":"running","termination":null}) {
            return Err("fresh-target 请求签发时 RustFS 或 Redis 未保持存活".into());
        }
        query::dispatch(
            backend,
            "service-guard",
            &json!({"value":self.context,"running":null}),
        )?;
        for descriptor in [
            &self.storage["rustfs"]["process_receipt"],
            &self.storage["rustfs"]["launch_receipt"],
            &self.registered["evidence"]["redis_runtime"],
        ] {
            common::bound_file(backend, descriptor, "当前服务运行收据")?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/fixture_control/request.rs"]
mod tests;
