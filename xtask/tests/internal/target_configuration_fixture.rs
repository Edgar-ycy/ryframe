use std::{path::Path, process::Command};

use serde_json::json;

pub(crate) fn python_command(script: &str) -> Command {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut command = Command::new(crate::process::resolved_executable(root, "python").unwrap());
    let tool = Path::new(env!("CARGO_BIN_EXE_xtask"));
    let digest = crate::recovery::clone::native::digest::file(tool).unwrap();
    let binding = json!({
        "format_version": 1, "kind": "ryframe-native-rate-limit-tool",
        "path": tool, "bytes": digest.bytes, "sha256": digest.sha256,
    })
    .to_string();
    command.args([
        "-X",
        "utf8",
        "-B",
        "-c",
        r#"import sys
from pathlib import Path
root, descriptor, source = sys.argv[1:4]
del sys.argv[1:4]
sys.path.insert(0, str(Path(root) / 'tools/python'))
from rate_limit_tool import install_binding
install_binding(descriptor)
exec(compile(source, '<string>', 'exec'), globals())
"#,
    ]);
    command.arg(root).arg(binding).arg(script);
    command
}
