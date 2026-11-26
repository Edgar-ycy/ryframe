use std::io::{Cursor, Write};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zip::{ZipWriter, write::SimpleFileOptions};

use super::{model, workbook};

pub(super) const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

pub(super) fn archive(members: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, text) in members {
        writer
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

pub(super) fn template(paths: &[&str]) -> Value {
    let mut sheet = format!("<worksheet xmlns=\"{NS}\"><sheetData><row>");
    for (index, path) in paths.iter().enumerate() {
        sheet.push_str(&format!(
            "<c r=\"A{}\" t=\"inlineStr\"><is><t>{path}</t></is></c>",
            index + 2
        ));
    }
    sheet.push_str("</row></sheetData></worksheet>");
    let content = archive(&[("xl/worksheets/sheet2.xml", &sheet)]);
    json!({"bytes":content.len(),"base64":STANDARD.encode(&content),"sha256":hex::encode(Sha256::digest(&content)),
        "media_type":"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"})
}

pub(super) fn target() -> Value {
    json!({"slot":"tenant-01","tenant_id":"101","admin":{"username":"admin"}})
}

pub(super) fn action() -> Value {
    json!({"target":{"slot":"tenant-01","tenant_id":"101"},"body":{"name":"固定部门","parent_id":null,"sort":0},
        "expected":{"name":"固定部门","parent_id":null,"ancestors":"0","sort":0,"status":"1","remark":null},
        "principal":{"subject_id":"9001","tenant_id":"101","username":"admin"},
        "template_principal":{"subject_id":"20000","tenant_id":"101","username":"user"},
        "before":{"total":0,"departments":[]},"before_template_paths":[]})
}

pub(super) fn row() -> Value {
    json!({"id":"5000","name":"固定部门","parent_id":null,"ancestors":"0","sort":0,"status":"1","remark":null,"created_at":"2026-10-23T00:00:00Z"})
}

pub(super) fn auth(write: bool) -> Value {
    let mut permissions = vec!["system:dept:list"];
    if write {
        permissions.push("system:dept:add");
    }
    json!({"subject_id":"9001","tenant_id":"101","username":"admin","is_super_admin":false,"permissions":permissions})
}

pub(super) fn observed(after: bool) -> Value {
    let rows = if after { vec![row()] } else { vec![] };
    json!({"list":{"response":{"code":200,"data":{"items":rows,"page":1,"page_size":100,"total":rows.len(),
        "total_pages":u64::from(after),"max_page_size":100}},"authorization":auth(false)},
        "detail":if after { json!({"response":{"code":200,"data":row()},"authorization":auth(false)}) } else {Value::Null},
        "template":{"response":template(if after { &["固定部门"] } else { &[] }),
            "authorization":{"subject_id":"20000","tenant_id":"101","username":"user","is_super_admin":false,"permissions":["system:user-import:add"]}}})
}

#[test]
fn complete_images_distinguish_before_after_and_unregistered_template_changes() {
    let action = action();
    assert_eq!(
        model::classify(&action, &observed(false), &target()).unwrap(),
        "before"
    );
    assert_eq!(
        model::classify(&action, &observed(true), &target()).unwrap(),
        "after"
    );
    let mut value = observed(true);
    value["template"]["response"] = template(&["其他", "固定部门"]);
    assert_eq!(
        model::classify(&action, &value, &target()).unwrap(),
        "mismatch"
    );
    value = observed(false);
    value["detail"] = observed(true)["detail"].clone();
    assert!(model::classify(&action, &value, &target()).is_err());
    value = observed(true);
    value["detail"]["response"]["data"]["status"] = json!("0");
    assert!(model::image(&action, &value, &target()).is_err());
}

#[test]
fn image_rejects_bad_ids_wildcards_subject_changes_and_partial_pages() {
    for id in [
        json!(5000),
        json!("0"),
        json!("01"),
        json!("9223372036854775808"),
        json!("-1"),
    ] {
        let mut value = observed(true);
        value["list"]["response"]["data"]["items"][0]["id"] = id;
        assert!(model::image(&action(), &value, &target()).is_err());
    }
    for field in ["page", "total", "total_pages", "max_page_size"] {
        let mut value = observed(false);
        value["list"]["response"]["data"][field] = json!(true);
        assert!(model::image(&action(), &value, &target()).is_err());
    }
    let mut inconsistent_page = observed(false);
    inconsistent_page["list"]["response"]["data"]["total_pages"] = json!(1);
    assert!(model::image(&action(), &inconsistent_page, &target()).is_err());
    for segment in ["list", "template"] {
        let mut value = observed(false);
        value[segment]["authorization"]["permissions"] =
            json!(["*", "system:dept:list", "system:user-import:add"]);
        value[segment]["authorization"]["is_super_admin"] = json!(true);
        assert!(model::image(&action(), &value, &target()).is_err());
        value = observed(false);
        value[segment]["authorization"]["subject_id"] = json!("999");
        assert!(model::image(&action(), &value, &target()).is_err());
    }
}

#[test]
fn created_response_requires_write_permission_and_fixed_principal() {
    let mut response = json!({"response":{"code":200,"data":row()},"authorization":auth(true)});
    assert_eq!(
        model::created_response(&action(), &target(), &response).unwrap(),
        row()
    );
    response["authorization"] = auth(false);
    assert!(model::created_response(&action(), &target(), &response).is_err());
    response["authorization"] = auth(true);
    response["authorization"]["tenant_id"] = json!("102");
    assert!(model::created_response(&action(), &target(), &response).is_err());
}

#[test]
fn workbook_supports_prefixed_namespaces_shared_rich_text_and_numeric_entities() {
    let sheet = format!(
        "<s:worksheet xmlns:s=\"{NS}\"><s:sheetData><s:row><s:c r=\"A1\"><s:v>表头</s:v></s:c><s:c r=\"A2\" t=\"s\"><s:v>0</s:v></s:c><s:c r=\"B2\"><s:v>忽略</s:v></s:c><s:c r=\"A3\"><s:is><s:t>根&#x2F;子&amp;项</s:t></s:is></s:c></s:row></s:sheetData></s:worksheet>"
    );
    let strings =
        format!("<sst xmlns=\"{NS}\"><si><r><t>  固定</t></r><r><t>部门  </t></r></si></sst>");
    let bytes = archive(&[
        ("xl/worksheets/sheet2.xml", &sheet),
        ("xl/sharedStrings.xml", &strings),
    ]);
    assert_eq!(
        workbook::paths(&bytes).unwrap(),
        vec!["固定部门", "根/子&项"]
    );
}

#[test]
fn workbook_rejects_shared_index_traversal_duplicates_invalid_xml_and_unsorted_paths() {
    let sheet = format!("<worksheet xmlns=\"{NS}\"><c r=\"A2\" t=\"s\"><v>1</v></c></worksheet>");
    let strings = format!("<sst xmlns=\"{NS}\"><si><t>唯一字符串</t></si></sst>");
    for members in [
        vec![
            ("xl/worksheets/sheet2.xml", sheet.as_str()),
            ("xl/sharedStrings.xml", strings.as_str()),
        ],
        vec![
            ("xl/worksheets/sheet2.xml", sheet.as_str()),
            ("../outside", "escape"),
        ],
        vec![("xl/worksheets/sheet2.xml", "<worksheet>")],
        vec![("xl/worksheets/sheet2.xml", "<!DOCTYPE x><worksheet/>")],
    ] {
        assert!(workbook::paths(&archive(&members)).is_err());
    }
    for paths in [&["重复", "重复"][..], &["根", "固定"][..]] {
        assert!(workbook::template(&template(paths)).is_err());
    }
    let mut response = template(&[]);
    response["bytes"] = json!(true);
    assert!(workbook::template(&response).is_err());
    response = template(&[]);
    response["sha256"] = json!("0".repeat(64));
    assert!(workbook::template(&response).is_err());
}
