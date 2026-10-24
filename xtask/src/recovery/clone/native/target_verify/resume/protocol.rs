use super::*;

const DIAGNOSTICS: &[&str] = &[
    "fresh 目标阶段文件经过链接或 junction",
    "目标续作 attempt 无效",
    "目标续作状态请求无效",
    "库存续作 reset plan 不属于原固定计划",
    "库存续作必须保留唯一 reset plan 命令收据",
    "库存续作包含未知或无法归属的阶段证据",
    "库存续作 intent 没有绑定原只读失败",
    "库存续作失败收据不是只读失败",
    "库存续作包含挂起、重复或未知结果",
    "库存续作失败收据不属于原 intent",
    "库存续作失败尝试时间谱系重叠",
    "库存续作成功尝试时间谱系无效",
    "库存续作成功尝试早于既有失败尝试",
    "库存续作判定期间目标证据树发生变化",
    "库存失败收据类型、阶段或只读资格字段无效",
    "库存失败原始目标输出不属于固定四库",
    "库存失败命令收据字段或退出状态无效",
    "库存失败 stdout 采集错误字段无效",
    "库存失败 stdout 文件描述无效",
    "库存失败 stdout 文件越出采集目录或名称无效",
    "库存失败 stdout 文件绑定变化",
    "库存失败目录缺少唯一失败收据或错误包含成功收据",
    "库存失败原始四库输出不是连续固定前缀",
    "库存 after 输出不能越过未完成的 before",
    "库存失败来源绑定缺失或前后变化",
    "库存失败目录包含未知证据",
    "库存失败 stdout 文件被多个命令收据重复引用",
    "库存失败 stdout 文件集合与命令收据引用不匹配",
    "目标续作状态转换不能越过未确认操作",
    "目标续作收据状态与固定前像不同",
];

pub(in super::super) fn diagnostic(reason: &str) -> Option<&'static str> {
    DIAGNOSTICS
        .iter()
        .copied()
        .find(|message| *message == reason)
}

pub(in super::super) fn execute(bytes: &[u8]) -> Option<Result<Value>> {
    let value = match std::str::from_utf8(bytes)
        .map_err(Into::into)
        .and_then(|text| json_io::parse_fragment(text, "目标续作请求"))
    {
        Ok(value) => value,
        Err(error) => return Some(Err(error)),
    };
    let operation = value["operation"].as_str()?;
    let fields: &[&str] = match operation {
        "inventory-resume-state" => &["request_descriptor", "prepared_files", "owned_lock"],
        "inventory-resume-confirmed" => &["initialized"],
        "inventory-resume-records" => &["context", "success"],
        "resume-transition" => &["transition", "arguments"],
        _ => return None,
    };
    Some(dispatch(&value, operation, fields))
}

fn dispatch(value: &Value, operation: &str, fields: &[&str]) -> Result<Value> {
    let mut expected = vec!["format_version", "operation", "backend", "target"];
    expected.extend_from_slice(fields);
    json_io::exact(value, &expected, "目标续作请求")?;
    check(
        value["format_version"].as_u64() == Some(1),
        "目标续作状态请求无效",
    )?;
    let backend = Path::new(json_io::text(&value["backend"], "续作后端")?);
    let target = Path::new(json_io::text(&value["target"], "续作目标")?);
    crate::local_test_path::validate_local_test_path(
        target,
        backend,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    match operation {
        "inventory-resume-state" => inventory::state(
            backend,
            target,
            &value["request_descriptor"],
            &value["prepared_files"],
            value["owned_lock"]
                .as_bool()
                .ok_or("初始化锁标志必须为布尔值")?,
        ),
        "inventory-resume-confirmed" => Ok(
            json!({"confirmed":inventory::confirmed(backend, target, &value["initialized"], None)}),
        ),
        "inventory-resume-records" => {
            inventory_records::execute(backend, target, &value["context"], &value["success"])
        }
        "resume-transition" => transition::execute(
            backend,
            target,
            json_io::text(&value["transition"], "续作转换")?,
            &value["arguments"],
        ),
        _ => unreachable!(),
    }
}
