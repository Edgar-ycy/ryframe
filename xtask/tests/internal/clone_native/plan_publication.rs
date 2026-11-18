use super::*;

fn publish(fixture: &Fixture) -> (PathBuf, native::InspectedInput) {
    fixture.write();
    let output = fixture.input.with_file_name("plan.json");
    let inspected = native::inspect_input(&fixture.root, &fixture.input).unwrap();
    native::json::write_new(&output, &inspected.wrapper).unwrap();
    (output, inspected)
}

#[test]
fn fresh_plan_publication_accepts_the_unchanged_inputs_and_preserves_all_files() {
    let fixture = Fixture::new();
    let (output, inspected) = publish(&fixture);
    let before = snapshot(&fixture.root);
    assert_eq!(native::json::read(&output).unwrap(), inspected.wrapper);

    native::finish_new_plan(&fixture.root, &fixture.input, &output, &inspected).unwrap();

    assert_eq!(snapshot(&fixture.root), before);
}

#[derive(Clone, Copy, Debug)]
enum Drift {
    InputBytes,
    InputPath,
    Evidence,
    Sql,
    Object,
    Catalog,
    Output,
}

fn replace_one_byte(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    bytes[0] ^= 1;
    fs::write(path, bytes).unwrap();
}

fn catalog_comment(fixture: &Fixture, path: &Path) {
    let original = native::catalog::load(&fixture.root).unwrap();
    assert!(
        original
            .sources
            .iter()
            .any(|source| source.as_path() == path)
    );
    let mut bytes = fs::read(path).unwrap();
    bytes.extend_from_slice(b"\n-- fixture: publication check\n");
    fs::write(path, bytes).unwrap();
    let changed = native::catalog::load(&fixture.root).unwrap();
    assert_eq!(changed.control, original.control);
    assert_eq!(changed.tenant, original.tenant);
    assert_eq!(changed.sources, original.sources);
}

#[test]
fn fresh_plan_publication_rejects_drift_without_overwriting_any_file() {
    for drift in [
        Drift::InputBytes,
        Drift::InputPath,
        Drift::Evidence,
        Drift::Sql,
        Drift::Object,
        Drift::Catalog,
        Drift::Output,
    ] {
        let fixture = Fixture::new();
        let (output, inspected) = publish(&fixture);
        let published = snapshot(&fixture.root);
        let mut input = fixture.input.clone();
        let (changed, expected) = match drift {
            Drift::InputBytes => {
                let mut bytes = fs::read(&input).unwrap();
                bytes.extend_from_slice(b"\n ");
                fs::write(&input, bytes).unwrap();
                assert_eq!(native::json::read(&input).unwrap(), fixture.value);
                (input.clone(), "离线核验期间输入清单变化")
            }
            Drift::InputPath => {
                input = fixture.input.with_file_name("same-input.json");
                fs::copy(&fixture.input, &input).unwrap();
                assert_eq!(fs::read(&input).unwrap(), published[&fixture.input]);
                (input.clone(), "离线核验期间输入清单变化")
            }
            Drift::Evidence | Drift::Sql | Drift::Object => {
                let binding = match drift {
                    Drift::Evidence => &fixture.value["evidence"]["target_stopped"],
                    Drift::Sql => &fixture.value["databases"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|row| row["key"] == "shared-control")
                        .unwrap()["artifact"],
                    Drift::Object => &fixture.value["objects"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|row| row["bucket"] == "uploads")
                        .unwrap()["entries"][0]["artifact"],
                    _ => unreachable!(),
                };
                let path = fixture.export.join(binding["file"].as_str().unwrap());
                replace_one_byte(&path);
                assert_eq!(
                    fs::metadata(&path).unwrap().len(),
                    binding["bytes"].as_u64().unwrap()
                );
                (path, "复制证据已变化或未完整写入")
            }
            Drift::Catalog => {
                let path = fixture.root.join("sql/ryframe_config.sql");
                catalog_comment(&fixture, &path);
                (path, "schema、生成目录或业务目录发生变化")
            }
            Drift::Output => {
                let mut changed_wrapper = inspected.wrapper.clone();
                changed_wrapper["plan"]["copy_id"] = json!("changed-after-publication");
                native::json::write_replace(&output, &changed_wrapper).unwrap();
                (output.clone(), "完整验证结果签发前计划文件变化")
            }
        };
        let before_check = snapshot(&fixture.root);
        let delta = before_check
            .iter()
            .filter(|(path, bytes)| published.get(*path) != Some(*bytes))
            .map(|(path, _)| path)
            .collect::<Vec<_>>();
        assert_eq!(delta, [&changed], "{drift:?}");
        let error =
            native::finish_new_plan(&fixture.root, &input, &output, &inspected).unwrap_err();
        assert!(error.to_string().contains(expected), "{drift:?}: {error}");
        assert_eq!(snapshot(&fixture.root), before_check, "{drift:?}");
    }
}
