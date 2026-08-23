use super::doctor::{ToolVersion, node_engine_satisfies, parse_python_version};

fn version(value: &str) -> ToolVersion {
    ToolVersion::parse(value, "测试版本").unwrap()
}

#[test]
fn parses_tool_versions() {
    assert_eq!(version("v22.22.2"), version("22.22.2"));
    assert!(ToolVersion::parse("22.22", "测试版本").is_err());
    assert_eq!(
        parse_python_version("Python 3.12.1\n").unwrap(),
        version("3.12.1")
    );
}

#[test]
fn validates_node_version_ranges() {
    let range = "^22.22.2 || >=24.15.0";
    assert!(node_engine_satisfies(version("22.22.2"), range).unwrap());
    assert!(!node_engine_satisfies(version("23.0.0"), range).unwrap());
    assert!(node_engine_satisfies(version("24.15.0"), range).unwrap());
}
