use ryframe_common::{define_excel_mapping, utils::excel::ExcelExporter};

#[test]
fn test_value_to_str() {
    assert_eq!(
        ExcelExporter::value_to_str(&serde_json::Value::String("hello".into())),
        "hello"
    );
    assert_eq!(
        ExcelExporter::value_to_str(&serde_json::Value::Number(serde_json::Number::from(42))),
        "42"
    );
    assert_eq!(
        ExcelExporter::value_to_str(&serde_json::Value::Number(
            serde_json::Number::from_f64(1.5).unwrap()
        )),
        "1.5"
    );
    assert_eq!(
        ExcelExporter::value_to_str(&serde_json::Value::Bool(true)),
        "true"
    );
    assert_eq!(
        ExcelExporter::value_to_str(&serde_json::Value::Bool(false)),
        "false"
    );
    assert_eq!(ExcelExporter::value_to_str(&serde_json::Value::Null), "");
    // 数组和对象转为 JSON 字符串
    let arr = serde_json::Value::Array(vec![serde_json::Value::Number(1.into())]);
    assert_eq!(ExcelExporter::value_to_str(&arr), "[1]");

    let obj = serde_json::json!({"key": "val"});
    assert_eq!(ExcelExporter::value_to_str(&obj), "{\"key\":\"val\"}");
}

#[test]
fn test_export_template() {
    let headers = &[("id", "ID"), ("name", "名称")];
    let result = ExcelExporter::export_template("测试", headers);
    assert!(result.is_ok());
    // 模板应包含 xlsx 文件头
    let buf = result.unwrap();
    assert!(!buf.is_empty());
    // xlsx 本质是 zip 文件，检查 PK 头
    assert_eq!(&buf[0..2], b"PK");
}

#[test]
fn test_define_excel_mapping_macro() {
    struct TestEntity;

    define_excel_mapping!(
        TestEntity,
        [("id", "编号"), ("name", "名称"), ("age", "年龄"),]
    );

    let headers = TestEntity::excel_headers();
    assert_eq!(headers.len(), 3);
    assert_eq!(headers[0], ("id", "编号"));
    assert_eq!(headers[1], ("name", "名称"));
    assert_eq!(headers[2], ("age", "年龄"));
}
