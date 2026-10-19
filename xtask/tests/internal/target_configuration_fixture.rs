use std::{path::Path, process::Command};

use serde_json::json;

pub(crate) use crate::recovery::clone::native::target_verify::input::configuration_fixture::toml_text;

pub(crate) fn python_command() -> Command {
    let mut command =
        Command::new(std::env::var_os("RYFRAME_PYTHON").unwrap_or_else(|| "python".into()));
    let tool = Path::new(env!("CARGO_BIN_EXE_xtask"));
    let digest = crate::recovery::clone::native::digest::file(tool).unwrap();
    command.env(
        "RYFRAME_RATE_LIMIT_TOOL_BINDING",
        json!({
            "format_version": 1, "kind": "ryframe-native-rate-limit-tool",
            "path": tool, "bytes": digest.bytes, "sha256": digest.sha256,
        })
        .to_string(),
    );
    command.args(["-X", "utf8", "-B"]);
    command
}
