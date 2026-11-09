use std::path::PathBuf;

use serde_json::{Value, json};

use crate::{
    Result,
    cli::CloneSide,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::clone::native::{
        digest,
        input::name,
        json as document,
        resources::common,
        target_verify::{database::model, input::request, storage::StorageInputs},
    },
};

pub(super) struct Inputs {
    pub(super) backend: PathBuf,
    pub(super) side: CloneSide,
    pub(super) port: Value,
}

pub(super) fn directory(value: &Value) -> Result<PathBuf> {
    let path = crate::recovery::source::evidence::binding::path(value)?;
    validate_absolute_path(&path, LocalTestPathKind::ExistingDirectory)?;
    Ok(path)
}

impl Inputs {
    pub(super) fn load(args: &Value, evidence: &std::path::Path) -> Result<Self> {
        let backend = directory(&args["backend"])?;
        let side = match args["side"].as_str() {
            Some("source") => CloneSide::Source,
            Some("target") => CloneSide::Target,
            _ => return Err("库存必须明确选择source或target".into()),
        };
        let value = Self {
            port: json!({"target":args["plan"][side.as_str()],"tools":args["plan"]["tools"],
                "maintenance_build":args["maintenance"],"execution_backend":{"path":backend},
                "config_dir":args["configuration"]["directory"]}),
            backend,
            side,
        };
        value.verify(args, evidence)?;
        Ok(value)
    }

    pub(super) fn verify(&self, args: &Value, evidence: &std::path::Path) -> Result<()> {
        if args["app_env"] != "test" || directory(&args["backend"])? != self.backend {
            return Err("库存必须绑定原执行后端与APP_ENV=test".into());
        }
        let plan = document::object(&args["plan"], "库存原始工具计划")?;
        if !plan.contains_key(self.side.as_str())
            || !plan.contains_key("tools")
            || plan
                .keys()
                .any(|key| !matches!(key.as_str(), "source" | "target" | "tools"))
        {
            return Err("库存只接受原两侧声明与工具计划".into());
        }
        let tools = document::object(&plan["tools"], "库存工具")?;
        if !tools.contains_key("mysql")
            || tools
                .keys()
                .any(|key| !matches!(key.as_str(), "mysql" | "mysqldump" | "node"))
        {
            return Err("库存工具只接受已绑定MySQL、dump与Node文件".into());
        }
        for value in tools.values() {
            request::external_file(value)?;
        }
        for side in ["source", "target"] {
            if let Some(value) = plan.get(side) {
                selected_shape(value)?;
            }
        }
        selected(evidence, &plan[self.side.as_str()])?;
        let private = std::collections::BTreeMap::from([("APP_ENV".into(), "test".into())]);
        let context = StorageInputs {
            backend: evidence,
            request: &self.port,
            environment: &private,
            observation_dir: evidence,
        };
        model::databases(&context)?;
        common::bound_file(evidence, &args["maintenance"], "库存维护构建")?;
        super::super::binding::source_configuration(
            &context,
            Some(&args["configuration"]),
            self.side,
        )?;
        if args["configuration"] != plan[self.side.as_str()]["s3"]["configuration"] {
            return Err("库存配置必须与所选侧的原文件配置绑定一致".into());
        }
        Ok(())
    }
}

fn selected(evidence: &std::path::Path, value: &Value) -> Result<()> {
    name(document::text(&value["scope_id"], "库存scope")?)?;
    let runtime = crate::recovery::source::evidence::binding::path(&value["runtime_dir"])?;
    validate_local_test_path(&runtime, evidence, LocalTestPathKind::StateDirectory)?;
    common::endpoint(document::text(&value["api_url"], "库存API端点")?)?;
    let database_name = regex::Regex::new(r"^[a-z0-9_]{1,64}$")?;
    for db in document::array(&value["databases"], "库存数据库")? {
        let database = document::text(&db["database"], "物理库名")?;
        let id = document::text(&db["server_uuid"], "数据库UUID")?;
        if !database_name.is_match(database)
            || uuid::Uuid::parse_str(id)
                .map_err(|_| "数据库UUID无效")?
                .to_string()
                != id
        {
            return Err("库存物理库名或UUID无效".into());
        }
        let file = crate::recovery::source::evidence::binding::path(&db["defaults_file"])?;
        validate_local_test_path(&file, evidence, LocalTestPathKind::ExistingFile)?;
        if digest::file(&file)?.sha256 != document::text(&db["defaults_sha256"], "MySQL文件摘要")?
        {
            return Err("库存MySQL文件绑定变化".into());
        }
    }
    Ok(())
}

fn selected_shape(value: &Value) -> Result<()> {
    document::exact(
        value,
        &["scope_id", "runtime_dir", "api_url", "s3", "databases"],
        "库存所选侧",
    )?;
    document::exact(&value["s3"], &["configuration"], "库存对象文件配置")?;
    document::exact(
        &value["s3"]["configuration"],
        &["app_env", "directory", "files", "sha256"],
        "库存文件配置绑定",
    )?;
    for binding in
        document::object(&value["s3"]["configuration"]["files"], "库存配置文件摘要")?.values()
    {
        document::exact(binding, &["bytes", "sha256"], "库存配置文件摘要")?;
    }
    for db in document::array(&value["databases"], "库存数据库")? {
        document::exact(
            db,
            &[
                "key",
                "kind",
                "mode",
                "database",
                "server_uuid",
                "defaults_file",
                "defaults_sha256",
            ],
            "库存数据库",
        )?;
    }
    Ok(())
}
