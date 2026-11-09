use serde_json::Value;

fn literal(value: &Value) -> String {
    format!(
        "CONVERT(0x{} USING utf8mb4)",
        hex::encode(value.as_str().expect("已校验确定性文字").as_bytes())
    )
}

pub(super) fn statement(rows: &[Value], first: i64) -> String {
    let values = rows
        .iter()
        .enumerate()
        .map(|(offset, row)| {
            format!(
                "({},{},{},{},{},'1',NULL,'0')",
                first + offset as i64,
                literal(&row["tenant_id"]),
                literal(&row["name"]),
                literal(&row["code"]),
                row["sort"]
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!(
        "SET SESSION time_zone = '+00:00'; START TRANSACTION;\nINSERT INTO `sys_post` (`id`,`tenant_id`,`name`,`code`,`sort`,`status`,`remark`,`del_flag`) VALUES\n{values};\nCOMMIT;\n"
    )
}
