use std::collections::BTreeMap;

use super::native::{
    catalog::Catalog,
    sql::{SqlValue, parse_row},
};
use serde_json::json;

#[test]
fn complete_sql_rows_preserve_literals_and_reject_invalid_dump_syntax() {
    let catalog = Catalog::from([(
        "biz_order".into(),
        BTreeMap::from([
            ("amount".into(), "DECIMAL".into()),
            ("payload".into(), "JSON".into()),
            ("optional".into(), "VARCHAR".into()),
            ("bytes".into(), "BLOB".into()),
            ("note".into(), "TEXT".into()),
        ]),
    )]);
    let decimal = "9007199254740993.12345678901234567890";
    let line = r#"INSERT INTO `biz_order` (`amount`, `payload`, `optional`, `bytes`, `note`) VALUES (9007199254740993.12345678901234567890,'{"key":[true,null,"中文"]}',NULL,0x00fFa1,_binary'中文,(x);\\quote\'\"\0\b\n\r\t\Z\%\_');"#;
    let expected = BTreeMap::from([
        ("amount".into(), SqlValue::Number(decimal.into())),
        (
            "payload".into(),
            SqlValue::Json(json!({"key":[true,null,"中文"]})),
        ),
        ("optional".into(), SqlValue::Null),
        ("bytes".into(), SqlValue::Bytes(vec![0, 255, 161])),
        (
            "note".into(),
            SqlValue::Text("中文,(x);\\quote'\"\0\u{8}\n\r\t\u{1a}\\%\\_".into()),
        ),
    ]);
    // 数值保留原始文本，包括原模式允许的 Unicode 数字，不经过浮点转换。
    for number in [decimal, "-0.000", "+1.", ".50", "6.02e+23", "١٢.٣"] {
        let mut row = expected.clone();
        row.insert("amount".into(), SqlValue::Number(number.into()));
        assert_eq!(
            parse_row(&line.replace(decimal, number), &catalog).unwrap(),
            ("biz_order".into(), row),
            "{number}"
        );
    }
    let mut null_json = expected;
    null_json.insert("payload".into(), SqlValue::Null);
    assert_eq!(
        parse_row(
            &line.replace(r#"'{"key":[true,null,"中文"]}'"#, "NULL"),
            &catalog
        )
        .unwrap(),
        ("biz_order".into(), null_json)
    );
    for (risk, invalid) in [
        ("重复列", line.replacen("`note`", "`bytes`", 1)),
        (
            "缺列",
            line.replacen("`optional`, ", "", 1)
                .replacen("NULL,", "", 1),
        ),
        ("未知列", line.replacen("`note`", "`unknown`", 1)),
        (
            "未知表",
            line.replacen("`biz_order`", "`outside_catalog`", 1),
        ),
        (
            "非规范列分隔",
            line.replacen("`amount`, `payload`", "`amount`,`payload`", 1),
        ),
        ("列值数量不同", line.replacen("NULL,", "", 1)),
        ("多语句", format!("{line} SELECT 1;")),
        ("多行值", line.replace(");", "),(1);")),
        ("缺分号", line.trim_end_matches(';').to_owned()),
        ("非INSERT", line.replacen("INSERT INTO", "REPLACE INTO", 1)),
        ("表达式", line.replace(decimal, "1+2")),
        ("非有限数字", line.replace(decimal, "NaN")),
        ("不完整指数", line.replace(decimal, "1e+")),
        ("奇数hex", line.replace("0x00fFa1", "0xF")),
        ("非法hex", line.replace("0x00fFa1", "0xGG")),
        ("不支持转义", line.replace(r"\n", r"\q")),
        (
            "未闭合字符串",
            line.strip_suffix("');").unwrap().to_owned() + ");",
        ),
        (
            "无效JSON",
            line.replace(r#"'{"key":[true,null,"中文"]}'"#, "'{bad}'"),
        ),
        (
            "重复JSON键",
            line.replace(r#"'{"key":[true,null,"中文"]}'"#, r#"'{"a":1,"a":2}'"#),
        ),
        (
            "非有限JSON数字",
            line.replace(r#"'{"key":[true,null,"中文"]}'"#, r#"'{"a":NaN}'"#),
        ),
        (
            "非字符串JSON",
            line.replace(r#"'{"key":[true,null,"中文"]}'"#, "1"),
        ),
    ] {
        assert!(parse_row(&invalid, &catalog).is_err(), "{risk}");
    }
}
