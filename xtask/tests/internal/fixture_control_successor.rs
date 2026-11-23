use std::{fs, path::PathBuf};

use super::{Fixture, command, create_directory_link, parse_control, root_dir, text, words};
use crate::cli::{FixtureControlCommand, FixtureSuccessorCommand, RecoveryCommand};

#[test]
fn parses_successor_relationship() {
    let fixture = Fixture::new();
    let mut relationship = Vec::new();
    for name in [
        "--source-result",
        "--predecessor-review",
        "--predecessor-request",
        "--successor-review",
        "--seed-request",
        "--base-request",
        "--candidate-request",
    ] {
        relationship.extend([name.to_owned(), text(&fixture.input)]);
    }
    relationship.extend([
        "--id".to_owned(),
        "successor-r24".to_owned(),
        "--output".to_owned(),
        text(&fixture.path("relationship.json")),
        "--write".to_owned(),
    ]);
    assert!(
        parse_control(command(
            "successor",
            Some("relationship"),
            relationship[..relationship.len() - 1].to_vec()
        ))
        .is_err()
    );
    let parsed = parse_control(command("successor", Some("relationship"), relationship)).unwrap();
    assert!(matches!(
        parsed,
        FixtureControlCommand::Successor(FixtureSuccessorCommand::Relationship(_))
    ));
}

#[test]
fn parses_successor_generation_preview_and_write_pairs() {
    let fixture = Fixture::new();
    let generation = fixture.generation_options();
    let planned = parse_control(command(
        "successor",
        Some("generation-request"),
        generation.clone(),
    ))
    .unwrap();
    assert!(matches!(
        planned,
        FixtureControlCommand::Successor(FixtureSuccessorCommand::GenerationRequest(ref value))
            if value.output.is_none()
    ));
    let FixtureControlCommand::Successor(FixtureSuccessorCommand::GenerationRequest(options)) =
        &planned
    else {
        unreachable!()
    };
    assert_eq!(options.config_dir, root_dir().join("config"));
    assert!(options.execution_source.is_none());
    let mut collision = options.clone();
    collision.output = Some(fixture.input.clone());
    let before = fs::read(&fixture.input).unwrap();
    assert!(
        crate::recovery::source::evidence::generation_request::run(&root_dir(), &collision)
            .is_err()
    );
    assert_eq!(fs::read(&fixture.input).unwrap(), before);
    let mut missing_configuration = generation.clone();
    missing_configuration.truncate(missing_configuration.len() - 2);
    assert!(
        parse_control(command(
            "successor",
            Some("generation-request"),
            missing_configuration
        ))
        .is_err()
    );
    let mut adapter = generation.clone();
    adapter.extend(words(&[
        "--adapter-contract",
        "legacy-stable-readiness-b0-v1",
        "--product-backend",
    ]));
    adapter.push(text(&root_dir()));
    let adapter = parse_control(command("successor", Some("generation-request"), adapter)).unwrap();
    assert!(
        matches!(adapter, FixtureControlCommand::Successor(FixtureSuccessorCommand::GenerationRequest(ref value)) if value.product_backend.as_deref() == Some(root_dir().as_path()))
    );
    let mut invalid_name = generation.clone();
    *invalid_name
        .iter_mut()
        .find(|value| value.as_str() == "source-r24")
        .unwrap() = "Source.R24".to_owned();
    assert!(
        parse_control(command(
            "successor",
            Some("generation-request"),
            invalid_name,
        ))
        .is_err()
    );
    let mut incomplete_adapter = generation.clone();
    incomplete_adapter.extend([
        "--adapter-contract".to_owned(),
        "legacy-stable-readiness-b0-v1".to_owned(),
    ]);
    assert!(
        parse_control(command(
            "successor",
            Some("generation-request"),
            incomplete_adapter,
        ))
        .is_err()
    );
    let mut unknown_adapter = generation.clone();
    unknown_adapter.extend([
        "--adapter-contract".to_owned(),
        "unregistered".to_owned(),
        "--product-backend".to_owned(),
        text(&fixture.controlled_dir),
    ]);
    assert!(
        parse_control(command(
            "successor",
            Some("generation-request"),
            unknown_adapter,
        ))
        .is_err()
    );
    let mut published = generation;
    published.extend([
        "--output".to_owned(),
        text(&fixture.path("generation.json")),
        "--write".to_owned(),
    ]);
    assert!(parse_control(command("successor", Some("generation-request"), published)).is_ok());
}

#[test]
fn successor_generation_keeps_source_and_evidence_path_boundaries() {
    let fixture = Fixture::new();
    let source = fixture.path("源码 directory");
    fs::create_dir(&source).unwrap();
    let link = fixture.path("linked source");
    create_directory_link(&source, &link);
    let mut valid = fixture.generation_options();
    valid[3] = text(&source);
    let parsed = parse_control(command(
        "successor",
        Some("generation-request"),
        valid.clone(),
    ))
    .unwrap();
    assert!(matches!(
        parsed,
        FixtureControlCommand::Successor(FixtureSuccessorCommand::GenerationRequest(_))
    ));
    for invalid in [
        PathBuf::from("relative"),
        fixture.input.clone(),
        fixture.path("missing"),
        link.clone(),
    ] {
        let mut arguments = valid.clone();
        arguments[3] = text(&invalid);
        assert!(
            parse_control(command("successor", Some("generation-request"), arguments)).is_err()
        );
    }
    #[cfg(unix)]
    fs::remove_file(&link).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&link).unwrap();
    for index in [1, 7, 9, 11] {
        let mut arguments = valid.clone();
        arguments[index] = text(&root_dir().join("Cargo.toml"));
        assert!(
            parse_control(command("successor", Some("generation-request"), arguments)).is_err()
        );
    }
    let mut outside_output = valid;
    outside_output.extend([
        "--output".to_owned(),
        text(&root_dir().join("forbidden-generation.json")),
        "--write".to_owned(),
    ]);
    assert!(
        parse_control(command(
            "successor",
            Some("generation-request"),
            outside_output
        ))
        .is_err()
    );
    assert!(!root_dir().join("forbidden-generation.json").exists());
}

#[test]
fn parses_successor_arm_request() {
    let fixture = Fixture::new();
    let arm = vec![
        "--successor".to_owned(),
        text(&fixture.input),
        "--source-export-result".to_owned(),
        text(&fixture.input_two),
        "--workspace".to_owned(),
        text(&fixture.controlled_dir),
        "--id".to_owned(),
        "arm-base-r24".to_owned(),
        "--side".to_owned(),
        "base".to_owned(),
        "--copy-directory".to_owned(),
        text(&fixture.path("copy base")),
    ];
    let preview = parse_control(command("successor", Some("arm-request"), arm.clone())).unwrap();
    assert!(matches!(
        preview,
        FixtureControlCommand::Successor(FixtureSuccessorCommand::ArmRequest(ref options))
            if options.output.is_none() && !options.independent_b1
    ));
    let independent = parse_independent_b1_arm_request(&arm);
    let output = fixture.path("native-arm-request.json");
    let mut write = arm.clone();
    write.push("--write".to_owned());
    assert!(parse_control(command("successor", Some("arm-request"), write)).is_err());
    let mut publish = arm;
    publish.extend(["--output".into(), text(&output)]);
    assert!(parse_control(command("successor", Some("arm-request"), publish.clone())).is_err());
    publish.push("--write".into());
    let parsed = parse_control(command("successor", Some("arm-request"), publish)).unwrap();
    for command in [preview, parsed, independent] {
        let error = crate::recovery::run(
            &RecoveryCommand::FixtureControl(Box::new(command)),
            &root_dir(),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "arm 选择的审阅关系不同于共享导出登记");
        assert!(!output.exists());
    }
}

fn parse_independent_b1_arm_request(arm: &[String]) -> FixtureControlCommand {
    let parse = |args| parse_control(command("successor", Some("arm-request"), args));
    let flag = "--independent-b1";
    let mut args = arm.to_vec();
    args.push(flag.into());
    assert!(
        parse(args.clone())
            .unwrap_err()
            .to_string()
            .contains("--side candidate")
    );

    args[9] = "candidate".into();
    let preview = parse(args.clone()).unwrap();
    assert!(matches!(
        preview,
        FixtureControlCommand::Successor(FixtureSuccessorCommand::ArmRequest(ref options))
            if options.output.is_none() && options.independent_b1
    ));
    let mut duplicate = args.clone();
    duplicate.push(flag.into());
    assert!(
        parse(duplicate)
            .unwrap_err()
            .to_string()
            .contains("不能重复")
    );

    let mut misplaced = args.clone();
    misplaced.pop();
    misplaced.insert(7, flag.into());
    assert!(
        parse(misplaced)
            .unwrap_err()
            .to_string()
            .contains("--id 缺少有效取值")
    );

    *args.last_mut().unwrap() = format!("{flag}=true");
    assert!(
        parse(args)
            .unwrap_err()
            .to_string()
            .contains("未知 fixture 控制参数")
    );
    assert!(
        parse_control(command(
            "successor",
            Some("generation-request"),
            vec![flag.into()]
        ))
        .unwrap_err()
        .to_string()
        .contains("未知 fixture 控制参数")
    );
    preview
}
