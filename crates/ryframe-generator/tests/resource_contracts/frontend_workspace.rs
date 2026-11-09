use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde::{Deserialize, Deserializer};
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Contract {
    permissions: String,
    operations: String,
    schema: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    format_version: u8,
    #[serde(deserialize_with = "optional_contract")]
    updated: Option<Contract>,
}

fn optional_contract<'de, D: Deserializer<'de>>(decoder: D) -> Result<Option<Contract>, D::Error> {
    Option::<Contract>::deserialize(decoder)
}

pub(super) fn register(source: &Path, frontend: &Path) {
    let paths = [
        frontend.join("src/api/generated/permissions.ts"),
        frontend.join("src/api/generated/operations/system.ts"),
        frontend.join("src/api/generated/schema/system.ts"),
    ];
    let input = json!({
        "format_version": 1,
        "permissions": fs::read_to_string(&paths[0]).expect("应读取候选权限清单"),
        "operations": fs::read_to_string(&paths[1]).expect("应读取候选 operation 清单"),
        "schema": fs::read_to_string(&paths[2]).expect("应读取候选 OpenAPI schema"),
    });
    let output = invoke(source, &serde_json::to_vec(&input).unwrap());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("前端应返回 JSON");
    assert!(value.is_object(), "响应必须是具名字段对象");
    assert!(value["updated"].is_null() || value["updated"].is_object());
    let response: Response =
        serde_json::from_slice(&output.stdout).expect("前端响应应符合严格协议");
    assert_eq!(response.format_version, 1);
    if let Some(updated) = response.updated {
        for (path, content) in
            paths
                .into_iter()
                .zip([updated.permissions, updated.operations, updated.schema])
        {
            fs::write(path, content).expect("应写入选中的临时前端契约");
        }
    }
}

fn invoke(source: &Path, input: &[u8]) -> Output {
    let source = std::path::absolute(source).expect("应解析前端绝对路径");
    let script = source.join("scripts/tests/helpers/resource-workspace-contract-cli.mjs");
    assert!(script.is_file(), "缺少配套前端的 Workspace 契约工具");
    let corepack = if cfg!(windows) {
        "corepack.cmd"
    } else {
        "corepack"
    };
    let backend = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cache = ryframe_config::ToolSettings::load(&backend)
        .unwrap()
        .pnpm_cache_dir(&backend)
        .unwrap();
    let mut command = Command::new(corepack);
    command.arg("pnpm");
    if let Some(cache) = cache {
        command.arg(format!("--config.cache-dir={}", cache.display()));
    }
    let mut child = command
        .args(["--config.verifyDepsBeforeRun=error", "exec", "node"])
        .arg(script)
        .current_dir(source)
        .env("COREPACK_ENABLE_NETWORK", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("应从已安装前端环境启动契约工具");
    let written = child.stdin.take().expect("应取得标准输入").write_all(input);
    let output = child.wait_with_output().expect("应等待并回收契约工具");
    assert!(written.is_ok(), "契约请求未完整传递：{written:?}");
    assert!(
        output.status.success(),
        "前端契约工具失败：{}；{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "前端契约工具产生意外标准错误输出");
    output
}
