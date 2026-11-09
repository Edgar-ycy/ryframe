use std::{collections::BTreeMap, fs, path::Path};

use axum::{body::Bytes, http::HeaderValue};
use serde_json::Value;

use super::Options;
use crate::{
    Result,
    check::consumer_context::HeldFile,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{clone::native::json as document, frontend_build},
};

pub(super) const RECEIPT: &str = ".vite/restore-build.json";

pub(super) struct Asset {
    pub(super) bytes: Bytes,
    pub(super) media: HeaderValue,
}

pub(super) struct Site {
    pub(super) assets: BTreeMap<String, Asset>,
}

impl Site {
    pub(super) fn load(options: &Options) -> Result<Self> {
        let root = validate_absolute_path(&options.frontend, LocalTestPathKind::ExistingDirectory)?;
        let dist =
            validate_absolute_path(&root.join("dist"), LocalTestPathKind::ExistingDirectory)?;
        let path = validate_absolute_path(&options.receipt, LocalTestPathKind::ExistingFile)?;
        let expected =
            validate_absolute_path(&dist.join(RECEIPT), LocalTestPathKind::ExistingFile)?;
        if fs::canonicalize(&path)? != fs::canonicalize(&expected)? {
            return Err("前端静态服务只接受同根 dist 的原构建收据".into());
        }
        let mut receipt = HeldFile::open(&expected)?;
        if receipt.sha256 != options.receipt_sha256 {
            return Err("前端静态服务构建收据 SHA-256 不同".into());
        }
        let bytes = receipt.read(document::MAX_JSON_BYTES)?;
        let original = document::parse_fragment(std::str::from_utf8(&bytes)?, "前端静态服务收据")?;
        if options.b0 {
            let proof = frontend_build::b0::validate(&root, None, &options.expected_head)?;
            Self::from_proof(&dist, &receipt, &original, &proof.receipt, || {
                proof.verify()
            })
        } else {
            let proof = frontend_build::validate(&root, None, Some(&options.expected_head))?;
            Self::from_proof(&dist, &receipt, &original, &proof.receipt, || {
                proof.verify()
            })
        }
    }

    fn from_proof(
        dist: &Path,
        receipt: &HeldFile,
        original: &Value,
        verified: &Value,
        recheck: impl FnOnce() -> Result<()>,
    ) -> Result<Self> {
        if original != verified {
            return Err("前端静态服务核验的收据与原 SHA-256 绑定不同".into());
        }
        let mut assets = BTreeMap::new();
        for row in document::array(&verified["files"], "前端静态服务产物清单")? {
            let relative = document::text(&row["path"], "前端静态服务产物路径")?;
            if !asset_path(relative) || relative == RECEIPT {
                return Err("前端静态服务产物路径不规范或包含私有收据".into());
            }
            let mut held = HeldFile::open(&dist.join(relative))?;
            let size = held.bytes;
            let expected = &row["bytes"];
            if (expected.as_u64() != Some(size) && expected.as_f64() != Some(size as f64))
                || row["sha256"].as_str() != Some(held.sha256.as_str())
            {
                return Err("前端静态服务产物字节或摘要与原收据不同".into());
            }
            let asset = Asset {
                bytes: Bytes::from(held.read(size)?),
                media: media(relative)?,
            };
            if assets.insert(relative.to_owned(), asset).is_some() {
                return Err("前端静态服务产物路径重复".into());
            }
        }
        if !assets.contains_key("index.html") {
            return Err("前端静态服务缺少已登记入口 index.html".into());
        }
        recheck()?;
        receipt.verify()?;
        Ok(Self { assets })
    }
}

fn asset_path(path: &str) -> bool {
    !path.is_empty()
        && !path.chars().any(|ch| ch == '\\' || ch.is_control())
        && !(cfg!(windows) && path.contains(':'))
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn media(relative: &str) -> Result<HeaderValue> {
    let mime = mime_guess::from_path(relative).first_or_octet_stream();
    let text = mime.as_ref();
    if text.starts_with("text/") || matches!(text, "application/javascript" | "application/json") {
        Ok(HeaderValue::from_str(&format!("{text}; charset=utf-8"))?)
    } else {
        Ok(HeaderValue::from_str(text)?)
    }
}
