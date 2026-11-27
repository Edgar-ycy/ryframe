use super::{Context, documents, fields};
use crate::{
    Result,
    recovery::{
        clone::native::{digest, resources::common, s3::app_configuration},
        inputs::domain,
    },
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Dataset {
    pub plan: Value,
    pub value: Value,
    pub post_samples: u64,
    pub files: u64,
}

pub(super) fn historical(bytes: u64, sha: &str, hash: &str) -> Result<()> {
    if bytes != 7222
        || sha != "7edf1f7fdff1aaa22dbe32ab524f42ad4425ddc7ad430dda833362a44ca719d5"
        || hash != "e2cd57cf6666cdef6ec40938e5d54d70e2168180783a718633e7b7639bb6ca75"
    {
        return Err("缺少恢复目标侧的参考计划不是已审计的C52历史证据".into());
    }
    Ok(())
}

fn reject_secrets(value: &Value) -> Result<()> {
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                if [
                    "password",
                    "admin_password",
                    "owner_password",
                    "access_key",
                    "secret_key",
                    "session_token",
                    "security_token",
                    "access_token",
                    "refresh_token",
                    "jwt_secret",
                ]
                .contains(&key.as_str())
                {
                    return Err("当前参考计划禁止内联凭据；使用明确配置文件绑定".into());
                }
                reject_secrets(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_secrets(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn plan(context: &mut Context, value: &Value, descriptor: &Value) -> Result<bool> {
    if value.get("target_side").is_none() {
        historical(
            descriptor["bytes"].as_u64().ok_or("历史计划字节数缺失")?,
            descriptor["sha256"].as_str().ok_or("历史计划摘要缺失")?,
            &digest::plan_hash(value)?,
        )?;
        return Ok(true);
    }
    reject_secrets(value)?;
    if value["format_version"] != 1
        || !matches!(value["target_side"].as_str(), Some("base" | "candidate"))
    {
        return Err("当前参考计划版本或目标侧无效".into());
    }
    domain::identifier(value["id"].as_str().ok_or("参考计划ID缺失")?)?;
    let work = super::super::binding::path(&value["work_dir"])?;
    crate::local_test_path::validate_local_test_path(
        &work,
        &context.root,
        crate::local_test_path::LocalTestPathKind::StateDirectory,
    )?;
    if let Some(dataset) = value.get("dataset") {
        let timeout = dataset["timeout_seconds"]
            .as_u64()
            .ok_or("数据集缺少明确超时")?;
        if !(1..=604800).contains(&timeout) {
            return Err("数据集超时超出范围".into());
        }
    }
    let mut identities = Vec::new();
    for side in ["source", "target"] {
        let scope = value[side]["scope_id"]
            .as_str()
            .ok_or("参考计划scope缺失")?;
        domain::scope_identifier(scope)?;
        let api = super::chain::endpoint(&value[side]["api_url"])?;
        if side == "target" {
            let worker = super::chain::worker_endpoint(&value[side]["worker_ready_url"])?;
            super::chain::endpoint(&value[side]["frontend_url"])?;
            if api == worker {
                return Err("源与目标必须逻辑相同、物理分离且探针不重叠".into());
            }
        }
        let runtime = super::super::binding::path(&value[side]["runtime_dir"])?;
        crate::local_test_path::validate_local_test_path(
            &runtime,
            &context.root,
            crate::local_test_path::LocalTestPathKind::StateDirectory,
        )?;
        documents::exact(
            &value[side]["s3"],
            &["endpoint", "region", "configuration"],
            "参考对象配置绑定",
        )?;
        super::chain::endpoint(&value[side]["s3"]["endpoint"])?;
        let region = value[side]["s3"]["region"]
            .as_str()
            .ok_or("参考对象region缺失")?;
        if region.is_empty()
            || !region
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err("参考对象region无效".into());
        }
        let mut snapshots = Vec::new();
        app_configuration::load(
            &context.root,
            &value[side]["s3"]["configuration"],
            &mut snapshots,
        )?;
        for (file, _) in snapshots {
            context.snapshots.push((file.clone(), digest::file(&file)?));
        }
        identities.push(databases(context, &value[side])?);
    }
    if value["source"]["scope_id"] == value["target"]["scope_id"]
        || identities[0].keys != identities[1].keys
        || !identities[0].physical.is_disjoint(&identities[1].physical)
        || !identities[0]
            .addresses
            .is_disjoint(&identities[1].addresses)
        || identities[0].modes != identities[1].modes
    {
        return Err("源与目标必须逻辑相同、物理分离且探针不重叠".into());
    }
    tools(context, value)?;
    Ok(false)
}

fn tools(context: &mut Context, value: &Value) -> Result<()> {
    for tool in ["mysql", "mysqldump"] {
        let file = super::super::binding::path(&value["tools"][tool]["path"])?;
        crate::local_test_path::validate_absolute_path(
            &file,
            crate::local_test_path::LocalTestPathKind::ExistingFile,
        )?;
        let hash = digest::file(&file)?;
        if value["tools"][tool]["sha256"] != hash.sha256 {
            return Err("参考外部工具摘要变化".into());
        }
        context.snapshots.push((file, hash));
    }
    Ok(())
}

struct Identities {
    keys: BTreeSet<String>,
    physical: BTreeSet<(String, String)>,
    modes: std::collections::BTreeMap<String, (Value, Value)>,
    addresses: BTreeSet<(String, u64, String)>,
}
fn databases(context: &mut Context, value: &Value) -> Result<Identities> {
    let mut keys = BTreeSet::new();
    let mut physical = BTreeSet::new();
    let mut kinds = std::collections::BTreeMap::new();
    let mut addresses = BTreeSet::new();
    let mut combined = 0;
    for db in documents::array(&value["databases"], "参考数据库目标")? {
        let key = db["key"].as_str().ok_or("数据库key缺失")?;
        domain::identifier(key)?;
        let database = db["database"].as_str().ok_or("数据库名缺失")?;
        domain::identifier(database)?;
        let uuid = db["server_uuid"].as_str().ok_or("数据库UUID缺失")?;
        domain::identifier(uuid)?;
        if !keys.insert(key.to_owned())
            || !physical.insert((uuid.to_owned(), database.to_ascii_lowercase()))
            || !matches!(db["kind"].as_str(), Some("combined" | "tenant"))
            || !matches!(db["mode"].as_str(), Some("shared" | "dedicated"))
            || (db["kind"] == "combined" && db["mode"] != "shared")
        {
            return Err("参考计划数据库目标、物理身份或模式重复无效".into());
        }
        combined += u64::from(db["kind"] == "combined");
        kinds.insert(key.to_owned(), (db["kind"].clone(), db["mode"].clone()));
        let file = super::super::binding::path(&db["defaults_file"])?;
        crate::local_test_path::validate_local_test_path(
            &file,
            &context.root,
            crate::local_test_path::LocalTestPathKind::ExistingFile,
        )?;
        let hash = digest::file(&file)?;
        if db["defaults_sha256"] != hash.sha256 {
            return Err("参考计划MySQL凭据摘要变化".into());
        }
        let connection = super::super::binding::summary(
            &serde_json::json!({"backend":context.root,"database":db}),
        )?;
        if !addresses.insert((
            connection["host"]
                .as_str()
                .ok_or("参考数据库主机缺失")?
                .to_owned(),
            connection["port"].as_u64().ok_or("参考数据库端口缺失")?,
            database.to_ascii_lowercase(),
        )) {
            return Err("参考数据库地址不能重复或冒充不同服务器".into());
        }
        context.snapshots.push((file, hash));
    }
    if keys.is_empty() || combined != 1 {
        return Err("参考环境必须恰有一个combined控制目标".into());
    }
    Ok(Identities {
        keys,
        physical,
        modes: kinds,
        addresses,
    })
}

pub(super) fn load(context: &mut Context, request: &Value) -> Result<Dataset> {
    let reference = context.read(&request["reference_plan"])?;
    let value = context.read(&request["dataset"])?;
    let historical = plan(context, &reference, &request["reference_plan"])?;
    let mut dataset_fields = fields::DATASET_FIELDS.to_vec();
    if !historical {
        dataset_fields.push("post_batch");
    }
    documents::exact(&value, &dataset_fields, "原始数据集")?;
    let scope = reference["source"]["scope_id"]
        .as_str()
        .ok_or("原参考计划缺少租户scope")?;
    let targets = documents::array(&value["tenants"], "原数据集租户")?;
    if value["format_version"] != 1
        || value["plan_sha256"] != digest::plan_hash(&reference)?
        || value["source_scope_id"] != scope
        || value["records"].as_u64().is_none_or(|value| value < 100000)
        || value["object_bytes"]
            .as_u64()
            .is_none_or(|value| value < 1024 * 1024 * 1024)
        || value["request_interval_ms"]
            .as_u64()
            .is_none_or(|value| !(1000..=5000).contains(&value))
        || targets.len() != 11
    {
        return Err("原数据集没有绑定原计划或未达到正式恢复规模".into());
    }
    let production = (!historical)
        .then(|| BatchPlan::load(&reference, &value))
        .transpose()?;
    let counts = counts(targets, scope, production.as_ref())?;
    if Some(counts.records) != value["records"].as_u64()
        || Some(counts.bytes) != value["object_bytes"].as_u64()
    {
        return Err("原数据集租户集合、记录总数或对象字节不一致".into());
    }
    if let Some(production) = production {
        production.verify_batch(context, &request["dataset"], &value, counts.batch_rows)?;
    }
    Ok(Dataset {
        plan: reference,
        value,
        post_samples: counts.posts,
        files: counts.files,
    })
}

#[derive(Default)]
struct Counts {
    bytes: u64,
    records: u64,
    files: u64,
    posts: u64,
    batch_rows: u64,
}

fn counts(targets: &[Value], scope: &str, production: Option<&BatchPlan>) -> Result<Counts> {
    let mut expected = (1..=10)
        .map(|index| (format!("{scope}-{index:02}"), index))
        .collect::<BTreeMap<_, _>>();
    expected.insert("system".into(), 0);
    let mut counts = Counts::default();
    let mut seen = BTreeSet::new();
    for tenant in targets {
        tenant_fields(tenant, production.is_none())?;
        let id = tenant["tenant_id"].as_str().ok_or("数据集租户身份无效")?;
        let index = expected.remove(id).ok_or("原数据集租户集合重复或不完整")?;
        if tenant["username"].as_str().is_none_or(str::is_empty) {
            return Err("原数据集租户集合重复或不完整".into());
        }
        let records = tenant["records"].as_u64().ok_or("租户记录数无效")?;
        counts.records = counts.records.checked_add(records).ok_or("记录数溢出")?;
        if let Some(production) = production {
            counts.batch_rows = counts
                .batch_rows
                .checked_add(production.tenant_batch_rows(tenant, index, records)?)
                .ok_or("岗位批次总数溢出")?;
        }
        let samples = documents::array(&tenant["posts"], "岗位样本")?;
        if samples.len() < 3 {
            return Err("原数据集岗位样本不足".into());
        }
        for post in samples {
            documents::exact(post, fields::POST_FIELDS, "岗位样本")?;
        }
        counts.posts += samples.len() as u64;
        let objects = documents::array(&tenant["files"], "业务对象样本")?;
        if objects.is_empty() {
            return Err("原数据集业务对象缺失".into());
        }
        for file in objects {
            documents::exact(file, fields::FILE_FIELDS, "业务对象样本")?;
            let key = file["file_path"].as_str().ok_or("业务对象路径无效")?;
            let sha = file["sha256"].as_str().ok_or("业务对象摘要无效")?;
            if !key.starts_with(&format!("{id}/"))
                || !seen.insert((id.to_owned(), key.to_owned()))
                || file["bytes"].as_u64().is_none_or(|value| value == 0)
                || sha.len() != 64
                || !sha
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err("原数据集对象样本重复或摘要无效".into());
            }
            counts.bytes = counts
                .bytes
                .checked_add(file["bytes"].as_u64().ok_or("对象字节数无效")?)
                .ok_or("对象字节溢出")?;
            counts.files += 1;
        }
    }
    if !expected.is_empty() {
        return Err("原数据集租户集合、记录总数或对象字节不一致".into());
    }
    Ok(counts)
}

struct BatchPlan {
    records: u64,
    api_per_tenant: u64,
    batch_size: u64,
}

impl BatchPlan {
    fn load(reference: &Value, dataset: &Value) -> Result<Self> {
        let settings = &reference["dataset"];
        let records = settings["records"]
            .as_u64()
            .filter(|value| (100_000..=1_000_000).contains(value))
            .ok_or("参考数据集计划的记录数无效")?;
        let api_per_tenant = settings["api_validation_posts"]
            .as_u64()
            .filter(|value| (1..=10).contains(value))
            .ok_or("参考数据集计划的 API 岗位数无效")?;
        let batch_size = settings["post_batch_rows"]
            .as_u64()
            .filter(|value| (100..=2000).contains(value))
            .ok_or("参考数据集计划的岗位批次大小无效")?;
        if dataset["records"] != records
            || dataset["request_interval_ms"] != settings["request_interval_ms"]
            || dataset["post_concurrency"] != 11
        {
            return Err("原数据集记录数、节拍或并发没有绑定参考计划".into());
        }
        Ok(Self {
            records,
            api_per_tenant,
            batch_size,
        })
    }

    fn tenant_batch_rows(&self, tenant: &Value, index: u64, records: u64) -> Result<u64> {
        let expected = self.records / 11 + u64::from(index < self.records % 11);
        let rows = expected - self.api_per_tenant;
        if records != expected
            || tenant["api_records"] != self.api_per_tenant
            || tenant["batch_records"] != rows
        {
            return Err("原数据集租户的 API 或批次岗位数与计划分配不同".into());
        }
        Ok(rows)
    }

    fn verify_batch(
        &self,
        context: &mut Context,
        dataset: &Value,
        value: &Value,
        rows: u64,
    ) -> Result<()> {
        let receipt = &value["post_batch"];
        documents::exact(receipt, fields::POST_BATCH_FIELDS, "原岗位批次收据")?;
        let (_, declared_rows) = crate::recovery::dataset_prepare::post_batch_range(receipt)?;
        let canonical_ids = ["first_id", "last_id"].into_iter().all(|field| {
            receipt[field]
                .as_str()
                .is_some_and(|id| id.bytes().all(|byte| byte.is_ascii_digit()))
        });
        if receipt["format_version"] != 1
            || !canonical_ids
            || receipt["plan_sha256"] != value["plan_sha256"]
            || declared_rows != rows
            || receipt["batch_rows"] != self.batch_size
            || receipt["batches"] != rows.div_ceil(self.batch_size)
        {
            return Err("原岗位批次收据没有绑定参考计划和完整行数".into());
        }
        let dataset = common::bound_file(&context.root, dataset, "原始数据集")?;
        let path = common::common_file(
            &context.root,
            &dataset.with_file_name("post-batch.ndjson"),
            "原岗位批次输入",
        )?;
        let metadata = std::fs::symlink_metadata(&path)?;
        let actual = digest::file(&path)?;
        if common::link_count_for(&path, &metadata)? != 1
            || actual.bytes == 0
            || receipt["input_sha256"] != actual.sha256
        {
            return Err("原岗位批次输入没有绑定同目录原件及其摘要".into());
        }
        context.snapshots.push((path, actual));
        Ok(())
    }
}

fn tenant_fields(tenant: &Value, historical: bool) -> Result<()> {
    let mut fields = fields::TENANT_FIELDS.to_vec();
    if historical {
        fields.push("password_env");
    } else {
        fields.extend(["api_records", "batch_records"]);
    }
    documents::exact(tenant, &fields, "原数据集租户")?;
    if historical
        && tenant["password_env"].as_str().is_none_or(|value| {
            value.is_empty()
                || !value.as_bytes()[0].is_ascii_uppercase()
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        })
    {
        return Err("历史数据集凭据标签无效".into());
    }
    Ok(())
}
