//! fresh 请求与审阅单侧资源共用原生事实校验，不授予历史请求执行权限。

use std::path::Path;

use serde_json::{Value, json};

use super::super::{digest, input, json as json_io};
use super::{
    evidence_model::{bound, check, read},
    input::{request, request_review},
};
use crate::Result;

pub(crate) fn validate(backend: &Path, value: &Value, predecessor: &Value) -> Result<Value> {
    json_io::exact(value, &request::fields(value), "新目标请求")?;
    request::config_directory(backend, value)?;
    check(
        value["format_version"].as_u64() == Some(1) && value["kind"] == "devex-clone-fresh-target",
        "新目标请求类型错误",
    )?;
    input::name(json_io::text(&value["id"], "新目标请求 ID")?)?;
    let descriptor = &value["review"];
    json_io::exact(
        descriptor,
        &["path", "bytes", "sha256", "canonical_sha256"],
        "审阅计划绑定",
    )?;
    let file = json!({"path":descriptor["path"],"bytes":descriptor["bytes"],"sha256":descriptor["sha256"]});
    let path = bound(backend, &file)?;
    let review = read(backend, &path)?;
    if predecessor.is_null() {
        request_review::validate(&review)?;
    } else {
        check(
            *descriptor == *predecessor,
            "历史 seed 请求未绑定指定 pending predecessor",
        )?;
        check(
            review["ready_for_execution"].as_bool() == Some(false),
            "历史 seed predecessor 必须保持 pending",
        )?;
        let mut structural = review.clone();
        structural["ready_for_execution"] = json!(true);
        request_review::validate(&structural)?;
    }
    check(
        digest::plan_hash(&review)? == input::digest_shape(&descriptor["canonical_sha256"])?,
        "审阅计划规范摘要不匹配",
    )?;
    let selected = review["scopes"]
        .get(json_io::text(&value["side"], "目标单侧")?)
        .ok_or("必须选择已就绪审阅计划的明确单侧")?;
    request::target(backend, &value["target"], selected)?;
    request::tools(&value["tools"], &review)?;
    json_io::exact(&value["storage"], &["rustfs", "redis"], "目标存储")?;
    bound(backend, &file)?;
    Ok(json!({"review":review,"selected":selected}))
}

pub(super) fn binaries(backend: &Path, value: &Value) -> Result<Value> {
    let binding = validate(backend, value, &Value::Null)?;
    let path = bound(backend, &value["maintenance_build"])?;
    let build = read(backend, &path)?;
    check(
        build["kind"] == "devex-clone-tool-build",
        "fresh 目标维护构建缺少完整二进制",
    )?;
    json_io::exact(
        &build["artifacts"],
        &["reset", "migrate", "tenant-data"],
        "维护构建二进制",
    )?;
    let mut binaries = json_io::object(&value["tools"], "目标工具")?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    // 后续恢复证据会按冻结 build.json 的原始字段顺序复核这组绑定；
    // 这里必须消费同一原始字节，不能采用 serde_json::Map 的排序结果。
    let build_bytes = std::fs::read(&path)?;
    for artifact in super::resume_model::artifact_bindings(&build_bytes)? {
        // 实际文件保护仍由调用方统一持有，原生读取不启动任何维护程序。
        json_io::text(&artifact["executable"], "维护二进制路径")?;
        input::digest_shape(&artifact["sha256"])?;
        binaries.push(json!({"path":artifact["executable"],"sha256":artifact["sha256"]}));
    }
    let rustfs = &value["storage"]["rustfs"];
    let redis = &value["storage"]["redis"];
    let storage = json!({"path":rustfs["identity"]["executable"],"sha256":rustfs["sha256"]});
    let review = &binding["review"];
    check(
        storage == review["tools"]["rustfs"],
        "fresh 目标存储二进制不属于审阅版本",
    )?;
    binaries.push(storage);
    if redis["platform"] == "windows" {
        check(
            redis["tool"] == review["tools"]["redis_server"],
            "fresh 目标 Windows Redis 工具不属于审阅版本",
        )?;
        binaries.extend(super::super::cache::windows_tool::package_bindings(
            &redis["tool"],
        )?);
    } else {
        check(
            redis["wsl"] == review["tools"]["wsl"],
            "fresh 目标存储二进制不属于审阅版本",
        )?;
        binaries.push(redis["wsl"].clone());
    }
    bound(backend, &value["maintenance_build"])?;
    Ok(json!({"bindings":binaries}))
}

pub(super) fn diagnostic(reason: &str) -> Option<&'static str> {
    // 固定错误映射保留字段和修复方向；不把工具路径、数据库密码或 JSON 原文带出进程。
    let message = match reason {
        "保护数据库 UUID必须为字符串" => {
            "历史来源或保护资源缺少有效 server_uuid；请核对数据库身份并重新审阅"
        }
        "审阅计划工具定义缺失" | "审阅工具路径必须为字符串" | "审阅 Redis 工具必须为字符串" => {
            "审阅计划工具定义缺失或类型无效；请补齐 mysql、mysqldump、rustfs 和 Redis 工具并重新审阅"
        }
        "审阅 Redis 服务缺失" => {
            "审阅计划服务定义无效：services.redis 缺失；请补齐服务定义并重新审阅"
        }
        "审阅 RustFS 缺失" => {
            "审阅计划服务定义无效：services.rustfs 缺失；请补齐服务定义并重新审阅"
        }
        "审阅计划类型错误或尚未就绪" => {
            "审阅计划类型错误或尚未就绪；请完成只读预检并重新绑定明确就绪的计划"
        }
        "三侧数据库相互重叠或碰到参考资源" => {
            "三侧数据库相互重叠或碰到参考资源；请为三侧配置独立物理数据库并重新审阅"
        }
        "审阅计划规范摘要不匹配" => {
            "审阅计划规范摘要不匹配；请核对 review 文件和 canonical_sha256，禁止绕过绑定"
        }
        "目标 MySQL 凭据文件变化" => {
            "目标 MySQL 凭据文件变化；请核对 defaults_file 和 defaults_sha256 并重新登记"
        }
        "目标工具实际文件与绑定不符" => {
            "目标工具实际文件与绑定不符；请恢复审阅工具或重新审阅当前二进制"
        }
        "目标工具不属于审阅版本" => {
            "目标工具不属于审阅版本；请核对 tools.mysql 的路径和摘要"
        }
        "新目标请求类型错误" => {
            "新目标请求类型错误；请使用当前 fresh-target 请求版本和类型"
        }
        "新目标必须精确包含四个逻辑数据库" => {
            "新目标必须精确包含四个逻辑数据库；请补齐目标 key 且禁止重复"
        }
        "数据库物理身份或凭据路径不是精确审阅目标" => {
            "数据库物理身份或凭据路径不是精确审阅目标；请核对数据库、UUID 和凭据文件"
        }
        "历史 seed 请求未绑定指定 pending predecessor" => {
            "历史 seed 请求未绑定指定 pending predecessor；请保留原请求和前驱收据，不授予执行权限"
        }
        "历史 seed predecessor 必须保持 pending" => {
            "历史 seed predecessor 必须保持 pending；请使用指定历史前驱且勿改写就绪状态"
        }
        "目标对象凭据必须在绑定文件中直接提供且不能为空" => {
            "目标对象凭据必须在绑定文件中直接提供且不能为空；请修正 target.s3 凭据字段"
        }
        "目标配置字段必须为非空无控制字符文本" => {
            "目标配置字段必须为非空无控制字符文本；请修正请求字段，不通过环境变量补齐"
        }
        "对象存储不是计划中的目标端点" => {
            "对象存储不是计划中的目标端点；请核对 target.s3.endpoint 和 region"
        }
        "必须选择已就绪审阅计划的明确单侧" => {
            "必须选择已就绪审阅计划的明确单侧；请核对 side 与审阅计划 scopes"
        }
        "新目标请求字段缺失或包含未知字段" => {
            "新目标请求字段缺失或包含未知字段；请按当前请求格式修正字段"
        }
        _ => return None,
    };
    Some(message)
}
