//! 重建已发布目标测试快照，保留正式夹具原有的重写和摘要行为。
use std::{fs, path::Path};

use serde_json::{Value, json};

use crate::{
    recovery::clone::native::{
        json as json_io, resources::common::binding, target_verify::tree as target_tree,
    },
    target_verify_fixture as fixture,
};

pub fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

pub fn complete() -> fixture::Fixture {
    let fixture = fixture::Fixture::new();
    let names = fixture::observations::prepared(&fixture);
    let files = target_tree::snapshot(&fixture.backend, &fixture.target, false).unwrap();
    let prepared_files = files
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| names.contains(item["path"].as_str().unwrap()))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(prepared_files.len(), names.len());
    let prepared_path = fixture.local.join("prepared-files.json");
    let mut prepared = json_io::read(&prepared_path).unwrap();
    prepared["files"] = json!(prepared_files);
    write(&prepared_path, &prepared);
    refresh(&fixture);
    fixture
}

pub fn refresh(fixture: &fixture::Fixture) {
    let path = fixture.local.join("initialized-files.json");
    let mut initialized = json_io::read(&path).unwrap();
    initialized["predecessor"] = binding(&fixture.local.join("prepared-files.json")).unwrap();
    initialized["files"] = target_tree::snapshot(&fixture.backend, &fixture.target, false).unwrap();
    write(&path, &initialized);
}
