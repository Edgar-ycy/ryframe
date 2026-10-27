//! 委托前端正式external-build；后端只负责输入窗口与原生产收据验证。
use super::{Port, source};
use crate::{
    Result,
    recovery::{
        clone::native::{digest, source as inventory},
        frontend_build,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) const TOOL_FILES: &[&str] = &[
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "vite.config.ts",
    "scripts/build-source-inventory.mjs",
    "scripts/build-source.mjs",
    "scripts/build-context-schema.mjs",
    "scripts/application-config.mjs",
    "scripts/application-config-toml.mjs",
    "scripts/application-config-files.mjs",
    "scripts/application-config-paths.mjs",
    "scripts/frontend-tool-config.mjs",
    "scripts/build-identity.mjs",
    "scripts/resource-render/json.mjs",
    "src/shared/config/apiEndpoint.ts",
    "scripts/restore-build.mjs",
];

pub(super) struct Built {
    pub(super) result: Value,
    proof: frontend_build::Proof,
    tools: std::path::PathBuf,
    inventory: inventory::Inventory,
    files: Vec<digest::FileDigest>,
}
impl Built {
    pub(super) fn verify(&self) -> Result<()> {
        if inventory::capture(&self.tools)? != self.inventory {
            return Err("前端构建工具来源在后续构建窗口发生漂移".into());
        }
        for (name, before) in TOOL_FILES.iter().zip(&self.files) {
            if digest::file(&digest::safe_file(&self.tools, name)?)? != *before {
                return Err("前端构建工具文件发生漂移".into());
            }
        }
        self.proof.verify()
    }
}

pub(super) fn run(
    tools: &Path,
    root: &Path,
    expected: &str,
    port: &mut impl Port,
) -> Result<Built> {
    let tools = source::repository(tools)?;
    let root = source::repository(root)?;
    let tools_before = inventory::capture(&tools)?;
    let before = inventory::capture(&root)?;
    if !tools_before.source.snapshot.clean
        || !before.source.snapshot.clean
        || before.source.snapshot.head != expected
    {
        return Err("前端工具与执行源码必须是干净来源，执行提交必须精确匹配".into());
    }
    let files = TOOL_FILES
        .iter()
        .map(|name| digest::safe_file(&tools, name).and_then(|path| digest::file(&path)))
        .collect::<Result<Vec<_>>>()?;
    for name in [
        "vite.config.ts",
        "scripts/application-config.mjs",
        "scripts/frontend-tool-config.mjs",
        "scripts/build-identity.mjs",
    ] {
        digest::safe_file(&root, name)?;
    }
    let receipt = root.join("dist/.vite/restore-build.json");
    let absent = match std::fs::symlink_metadata(&receipt) {
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error.into()),
    };
    if absent {
        port.command(
            &tools,
            "node",
            &[
                tools
                    .join("scripts/restore-build.mjs")
                    .to_str()
                    .ok_or("前端工具路径不是UTF8")?
                    .to_owned(),
                "external-build".into(),
                "--source-root".into(),
                root.to_str().ok_or("前端来源路径不是UTF8")?.to_owned(),
                "--expected-head".into(),
                expected.into(),
                "--write".into(),
            ],
        )?;
    }
    let proof = frontend_build::validate(
        &root,
        Some(&inventory::inventory_value(&before)),
        Some(expected),
    )?;
    if inventory::capture(&tools)? != tools_before || inventory::capture(&root)? != before {
        return Err("前端构建期间登记源码或受控工具变化".into());
    }
    for (name, before) in TOOL_FILES.iter().zip(&files) {
        if digest::file(&digest::safe_file(&tools, name)?)? != *before {
            return Err("前端构建工具文件在使用窗口发生漂移".into());
        }
    }
    proof.verify()?;
    Ok(Built {
        result: json!({"frontend_source":root,"frontend_receipt":receipt,"frontend_action":if absent {"built"} else {"verified"}}),
        proof,
        tools,
        inventory: tools_before,
        files,
    })
}
