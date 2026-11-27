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
use std::{fs, io::ErrorKind, path::Path};

const INVENTORY_TOOL: &str = "scripts/build-source-inventory.ts";
const DOMAINS_TOOL: &str = "scripts/build-source-domains.ts";
const LEGACY_INVENTORY_TOOL: &str = "scripts/build-source-inventory.mjs";
const JSON_TOOL: &str = "scripts/resource-render/json.ts";
const LEGACY_JSON_TOOL: &str = "scripts/resource-render/json.mjs";

pub(super) const TOOL_FILES: &[&str] = &[
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "vite.config.ts",
    INVENTORY_TOOL,
    DOMAINS_TOOL,
    "scripts/build-source.mjs",
    "scripts/build-context-schema.mjs",
    "scripts/application-config.mjs",
    "scripts/application-config-toml.mjs",
    "scripts/application-config-files.mjs",
    "scripts/application-config-paths.mjs",
    "scripts/frontend-tool-config.mjs",
    "scripts/build-identity.mjs",
    JSON_TOOL,
    "src/shared/config/apiEndpoint.ts",
    "scripts/restore-build.mjs",
];

pub(super) struct Built {
    pub(super) result: Value,
    verification: Verification,
}

enum Verification {
    Current(Box<Current>),
    B0(Box<frontend_build::b0::Proof>),
}

struct Current {
    proof: frontend_build::Proof,
    tools: std::path::PathBuf,
    inventory: inventory::Inventory,
    files: Vec<(&'static str, digest::FileDigest)>,
}
impl Built {
    pub(super) fn verify(&self) -> Result<()> {
        match &self.verification {
            Verification::Current(current) => current.verify(),
            Verification::B0(proof) => proof.verify(),
        }
    }
}

impl Current {
    fn verify(&self) -> Result<()> {
        if inventory::capture(&self.tools)? != self.inventory {
            return Err("前端构建工具来源在后续构建窗口发生漂移".into());
        }
        if capture_tool_files(&self.tools, &self.inventory)? != self.files {
            return Err("前端构建工具文件发生漂移".into());
        }
        self.proof.verify()
    }
}

fn capture_tool_files(
    tools: &Path,
    source: &inventory::Inventory,
) -> Result<Vec<(&'static str, digest::FileDigest)>> {
    let present = |path| source.files.iter().any(|file| file.path == path);
    // 以已复核的工具检出确定入口，执行检出可能来自另一代。
    let (legacy, unbound): (bool, &[&str]) = match (
        present(INVENTORY_TOOL),
        present(DOMAINS_TOOL),
        present(LEGACY_INVENTORY_TOOL),
    ) {
        (true, true, false) => (false, &[LEGACY_INVENTORY_TOOL]),
        (false, false, true) => (true, &[INVENTORY_TOOL, DOMAINS_TOOL]),
        _ => return Err("前端构建工具来源必须匹配完整且唯一的来源清单布局".into()),
    };
    for &path in unbound {
        require_absent_tool(tools, path, "前端构建工具目录存在未绑定的来源清单文件")?;
    }
    let (json_tool, other_json_tool) = match (present(JSON_TOOL), present(LEGACY_JSON_TOOL)) {
        (true, false) => (JSON_TOOL, LEGACY_JSON_TOOL),
        (false, true) => (LEGACY_JSON_TOOL, JSON_TOOL),
        _ => return Err("前端构建工具来源必须绑定唯一的 JSON 解析入口".into()),
    };
    require_absent_tool(
        tools,
        other_json_tool,
        "前端构建工具目录存在未绑定的 JSON 解析文件",
    )?;
    TOOL_FILES
        .iter()
        .filter_map(|&path| match (legacy, path) {
            (true, INVENTORY_TOOL) => Some(LEGACY_INVENTORY_TOOL),
            (true, DOMAINS_TOOL) => None,
            (_, JSON_TOOL) => Some(json_tool),
            _ => Some(path),
        })
        .map(|path| Ok((path, digest::file(&digest::safe_file(tools, path)?)?)))
        .collect()
}

fn require_absent_tool(root: &Path, path: &str, duplicate: &'static str) -> Result<()> {
    match fs::symlink_metadata(root.join(path)) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(duplicate.into()),
    }
}

pub(super) fn run_b0(root: &Path, expected: &str) -> Result<Built> {
    let root = source::repository(root)?;
    if expected != frontend_build::b0::HEAD {
        return Err("登记的 B0 适配仅允许原始固定前端提交".into());
    }
    let receipt = root.join("dist/.vite/restore-build.json");
    let (action, proof) = match std::fs::symlink_metadata(&receipt) {
        Ok(_) => (
            "verified",
            frontend_build::b0::validate(&root, None, expected)?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ("built", frontend_build::b0::build(&root)?)
        }
        Err(error) => return Err(error.into()),
    };
    Ok(Built {
        result: json!({"frontend_source":root,"frontend_receipt":receipt,"frontend_action":action}),
        verification: Verification::B0(Box::new(proof)),
    })
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
    let files = capture_tool_files(&tools, &tools_before)?;
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
    if capture_tool_files(&tools, &tools_before)? != files {
        return Err("前端构建工具文件在使用窗口发生漂移".into());
    }
    proof.verify()?;
    Ok(Built {
        result: json!({"frontend_source":root,"frontend_receipt":receipt,"frontend_action":if absent {"built"} else {"verified"}}),
        verification: Verification::Current(Box::new(Current {
            proof,
            tools,
            inventory: tools_before,
            files,
        })),
    })
}
