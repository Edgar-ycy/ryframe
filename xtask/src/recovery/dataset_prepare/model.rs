use super::documents;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Inputs {
    pub root: PathBuf,
    pub plan_path: PathBuf,
    pub plan: Value,
    pub side: String,
    pub preflight: Option<Value>,
    pub dataset: Option<Value>,
    pub password: String,
    pub initial_password: Option<String>,
    snapshots: Vec<(PathBuf, Vec<u8>)>,
}
impl Inputs {
    pub fn read(request: &Value, root: &Path) -> Result<Self> {
        documents::exact(
            request,
            &[
                "backend_dir",
                "format_version",
                "kind",
                "plan",
                "preflight",
                "side",
                "verify_existing",
                "write",
            ],
            "数据集请求",
        )?;
        if request["format_version"] != 1
            || request["kind"] != super::KIND
            || request["write"] != true
            || request["backend_dir"].as_str() != root.to_str()
        {
            return Err("数据集请求版本、后端目录或写入授权无效".into());
        }
        let mut snapshots = Vec::new();
        let plan_path = path(&request["plan"])?;
        let plan = load(&plan_path, root, &mut snapshots)?;
        let preflight = optional(&request["preflight"], root, &mut snapshots)?;
        let dataset = optional(&request["verify_existing"], root, &mut snapshots)?;
        let side = request["side"].as_str().ok_or("核验侧无效")?.to_owned();
        if preflight.is_some() == dataset.is_some()
            || !matches!(side.as_str(), "source" | "target")
            || preflight.is_some() && side != "source"
        {
            return Err("数据集请求必须明确唯一模式及有效侧".into());
        }
        specification(&plan)?;
        let work = path(&plan["work_dir"])?;
        validate_local_test_path(&work, root, LocalTestPathKind::ExistingDirectory)?;
        if let Some(value) = &preflight {
            verify_preflight(&plan, value)?;
        }
        if let Some(value) = &dataset {
            validate_dataset(&plan, value)?;
        }
        let password = super::configuration::current(root, &plan["dataset"], &mut snapshots)?;
        let initial_password = if preflight.is_some() {
            Some(super::configuration::initial(
                root,
                &plan["dataset"]["configuration"],
                &password,
                &mut snapshots,
            )?)
        } else {
            None
        };
        Ok(Self {
            root: root.into(),
            plan_path,
            plan,
            side,
            preflight,
            dataset,
            password,
            initial_password,
            snapshots,
        })
    }
    pub fn unchanged(&self) -> Result<()> {
        for (path, bytes) in &self.snapshots {
            validate_local_test_path(path, &self.root, LocalTestPathKind::ExistingFile)?;
            if fs::read(path)? != *bytes {
                return Err("数据集计划、预检或原收据在执行期间发生变化".into());
            }
        }
        super::configuration::current(&self.root, &self.plan["dataset"], &mut Vec::new())?;
        Ok(())
    }
    pub fn dataset_sha(&self) -> Result<String> {
        let (_, bytes) = self.snapshots.get(1).ok_or("缺少数据集快照")?;
        Ok(sha(bytes))
    }
}
fn load(path: &Path, root: &Path, snapshots: &mut Vec<(PathBuf, Vec<u8>)>) -> Result<Value> {
    validate_local_test_path(path, root, LocalTestPathKind::ExistingFile)?;
    let value = documents::read(path)?;
    let bytes = fs::read(path)?;
    if documents::parse_fragment(std::str::from_utf8(&bytes)?, "数据集输入")? != value {
        return Err("读取期间数据集输入变化".into());
    }
    snapshots.push((path.into(), bytes));
    Ok(value)
}
fn optional(
    value: &Value,
    root: &Path,
    snapshots: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<Option<Value>> {
    if value.is_null() {
        Ok(None)
    } else {
        load(&path(value)?, root, snapshots).map(Some)
    }
}
pub(super) fn path(value: &Value) -> Result<PathBuf> {
    let text = value
        .as_str()
        .filter(|v| !v.is_empty() && !v.contains(['\r', '\n', '\0']))
        .ok_or("数据集路径必须是无控制字符的绝对路径")?;
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err("数据集路径必须为绝对路径".into());
    }
    Ok(path)
}
pub(super) fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub(super) fn hash(value: &Value) -> Result<String> {
    Ok(sha(&serde_json::to_vec(value)?))
}
pub(super) fn integer(value: &Value, min: u64, max: u64, label: &str) -> Result<u64> {
    value
        .as_u64()
        .filter(|v| (min..=max).contains(v))
        .ok_or_else(|| format!("{label}超出整数范围").into())
}
pub(super) fn endpoint(value: &Value) -> Result<url::Url> {
    let url = url::Url::parse(value.as_str().ok_or("参考端点缺失")?)?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
        || url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("参考API与前端必须使用明确loopback根地址".into());
    }
    Ok(url)
}
pub(super) fn specification(plan: &Value) -> Result<()> {
    reject_credentials(plan)?;
    let id = plan["id"].as_str().ok_or("数据集ID缺失")?;
    if !regex::Regex::new(r"^[a-z][a-z0-9_-]{2,31}$")?.is_match(id) {
        return Err("数据集ID必须为3到32位小写安全标识".into());
    }
    let s = &plan["dataset"];
    integer(&s["request_interval_ms"], 1000, 5000, "请求节奏")?;
    integer(&s["api_validation_posts"], 1, 10, "API验证记录数")?;
    integer(&s["post_batch_rows"], 100, 2000, "岗位批次大小")?;
    integer(&s["records"], 100_000, 1_000_000, "参考业务记录")?;
    let objects = integer(&s["object_count"], 11, 1024, "对象数")?;
    let bytes = integer(&s["object_bytes"], 1024, 4 * 1024 * 1024, "对象大小")?;
    if objects * bytes < 1024 * 1024 * 1024 {
        return Err("参考对象必须合计至少1GiB".into());
    }
    if s["admin"]["tenant_id"] != "system"
        || s["admin"]["username"].as_str().is_none_or(str::is_empty)
        || s.get("owner_password_env").is_some()
        || s["admin"].get("password_env").is_some()
        || s.get("owner_password").is_some()
        || s["admin"].get("password").is_some()
    {
        return Err("数据集计划只允许配置绑定，不得包含密码或环境凭据字段".into());
    }
    documents::exact(
        &s["configuration"],
        &["app_env", "directory", "files", "sha256"],
        "数据集配置绑定",
    )?;
    documents::exact(
        &s["administrator_credentials"],
        &["path", "bytes", "sha256"],
        "当前管理员凭据绑定",
    )?;
    let targets = s["tenant_targets"]
        .as_array()
        .filter(|v| v.len() == 10)
        .ok_or("参考数据集必须包含十个普通租户")?;
    let databases = plan["source"]["databases"]
        .as_array()
        .ok_or("源数据库登记缺失")?;
    let mut modes = std::collections::BTreeSet::new();
    for target in targets {
        let db = databases
            .iter()
            .find(|db| db["key"] == *target)
            .ok_or("租户目标未登记")?;
        let mode = db["mode"].as_str().ok_or("租户目标模式缺失")?;
        modes.insert(mode);
        if mode == "dedicated" && targets.iter().filter(|v| *v == target).count() != 1 {
            return Err("独立目标只能分配一个普通租户".into());
        }
    }
    if !modes.contains("shared") || !modes.contains("dedicated") {
        return Err("参考租户必须覆盖共享和独立目标".into());
    }
    for side in ["source", "target"] {
        documents::exact(
            &plan[side]["s3"],
            &["endpoint", "region", "configuration"],
            "参考计划对象绑定；禁止access_key、secret_key和session token",
        )?;
        for key in ["api_url", "frontend_url"] {
            endpoint(&plan[side][key])?;
        }
    }
    Ok(())
}
fn reject_credentials(value: &Value) -> Result<()> {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                if matches!(
                    name.as_str(),
                    "password"
                        | "admin_password"
                        | "owner_password"
                        | "access_key"
                        | "secret_key"
                        | "session_token"
                        | "security_token"
                        | "access_token"
                        | "refresh_token"
                        | "jwt_secret"
                ) {
                    return Err("参考计划禁止包含凭据或会话令牌，只允许受管配置绑定".into());
                }
                reject_credentials(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_credentials(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub(super) fn verify_preflight(plan: &Value, receipt: &Value) -> Result<()> {
    if receipt["format_version"] != 1
        || receipt["kind"] != "restore-reference-dataset-preflight"
        || receipt["plan_sha256"] != hash(plan)?
        || receipt["side"] != "source"
        || receipt["scope_id"] != plan["source"]["scope_id"]
        || receipt["actions"]["business"] != "empty_source_verified"
    {
        return Err("数据准备预检收据未绑定当前计划、source侧或空源检查".into());
    }
    Ok(())
}
pub(super) fn tenant(plan: &Value, index: usize) -> String {
    if index == 0 {
        "system".into()
    } else {
        format!(
            "{}-{index:02}",
            plan["source"]["scope_id"].as_str().unwrap_or("")
        )
    }
}
pub(super) fn count(records: u64, index: usize) -> u64 {
    records / 11 + u64::from((index as u64) < records % 11)
}
pub(super) fn identity(plan: &Value, index: usize) -> Value {
    let admin = &plan["dataset"]["admin"];
    json!({"tenant_id":tenant(plan,index),"username":if index==0 {admin["username"].clone()} else {json!("owner")}})
}
pub(super) fn validate_dataset(plan: &Value, dataset: &Value) -> Result<()> {
    let digest_pattern = regex::Regex::new(r"^[a-f0-9]{64}$")?;
    let tenants = dataset["tenants"]
        .as_array()
        .filter(|v| v.len() == 11)
        .ok_or("旧数据租户收据不完整")?;
    if dataset["format_version"] != 1
        || dataset["plan_sha256"] != hash(plan)?
        || dataset["source_scope_id"] != plan["source"]["scope_id"]
    {
        return Err("旧数据收据没有绑定本次参考环境计划".into());
    }
    let mut expected = (0..11)
        .map(|i| tenant(plan, i))
        .collect::<std::collections::BTreeSet<_>>();
    for item in tenants {
        let id = item["tenant_id"].as_str().ok_or("租户身份缺失")?;
        let index = if id == "system" {
            0
        } else {
            id.rsplit('-')
                .next()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|v| (1..=10).contains(v))
                .ok_or("租户归属无效")?
        };
        if !expected.remove(id) || item["username"] != identity(plan, index)["username"] {
            return Err("旧数据租户重复或归属不符".into());
        }
        let posts = item["posts"]
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or("旧岗位样本缺失")?;
        for post in posts {
            positive_id(&post["id"])?;
            for key in ["code", "name"] {
                if post[key].as_str().is_none_or(str::is_empty) {
                    return Err("旧岗位样本无效".into());
                }
            }
        }
        for file in item["files"].as_array().ok_or("旧对象样本缺失")? {
            if file["file_path"].as_str().is_none_or(str::is_empty)
                || !digest_pattern.is_match(file["sha256"].as_str().unwrap_or(""))
            {
                return Err("旧对象样本无效".into());
            }
            integer(&file["bytes"], 1, 4 * 1024 * 1024, "旧对象大小")?;
        }
    }
    Ok(())
}
pub(super) fn positive_id(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|v| v.parse::<i64>().is_ok_and(|id| id > 0) && !v.starts_with('0'))
        .ok_or_else(|| "业务ID必须是正整数串".into())
}
