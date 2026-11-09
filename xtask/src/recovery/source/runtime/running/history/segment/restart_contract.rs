//! 历史异常合同由该次干净 producer 的 Git 对象确定，不从失败正文反选。
use super::*;
use crate::recovery::source::evidence::staging;

const PATHS: [&str; 3] = [
    "tools/python/devex_clone_storage_process.py",
    "tools/python/devex_clone_target_storage.py",
    "tools/python/devex_clone_target_evidence.py",
];

pub(super) struct RestartContract {
    pub(super) error_type: &'static str,
    pub(super) calls: [(&'static str, &'static str); 5],
}

pub(super) fn restart_contract(backend: &Path, row: &Value) -> Result<RestartContract> {
    staging::validate_source(&row["sources"])?;
    let snapshot = &row["sources"]["snapshot"];
    if snapshot["files"] != json!([])
        || snapshot["patch_sha256"] != crate::contract::sha256_hex(b"")
    {
        return Err("历史存储失败必须来自无补丁、无额外文件的干净 producer；核对原始来源".into());
    }
    let head = document::text(&snapshot["head"], "历史存储 producer HEAD")?;
    // 必须是 commit 对象，不能用任意 tree 对象冒充生产者提交。
    if staging::git(backend, &["cat-file", "-t", head])? != b"commit\n" {
        return Err("历史 producer HEAD 不是提交对象；恢复完整原始 Git 对象后再核验".into());
    }
    let observed = PATHS
        .iter()
        .map(|path| blob(backend, head, path))
        .collect::<Result<Vec<_>>>()?;
    let old = [
        Some("c695960fd7d165f039aa2046902c3aa2972fb803"),
        Some("368d95c120c82967620a5f9431472b10a3d51875"),
        Some("2a7359740d9f84fb78afd10b861e4d58b8b40b29"),
    ];
    let native = [
        Some("16684182d9f03e3ee6be7bb4b7c10ece435de77d"),
        None,
        Some("65055d21c98ce322926190227353c672733779a8"),
    ];
    let actual = observed.iter().map(Option::as_deref).collect::<Vec<_>>();
    let (error_type, last) = if actual == old {
        (
            "CalledProcessError",
            ("devex_clone_target_storage", "actual_windows_argv"),
        )
    } else if actual == native {
        ("ValueError", ("devex_clone_target_evidence", "evidence"))
    } else {
        return Err("历史存储 producer 三文件组合未知或混配；保留现场并核对原始工具来源".into());
    };
    Ok(RestartContract {
        error_type,
        calls: [
            ("devex_clone_run", "execute"),
            ("devex_clone_storage", "execute_storage"),
            ("devex_clone_storage_process", "start"),
            ("devex_clone_storage_process", "actual_arguments"),
            last,
        ],
    })
}

fn blob(backend: &Path, head: &str, path: &str) -> Result<Option<String>> {
    let tree = staging::git(backend, &["ls-tree", "-z", head, "--", path])?;
    if tree.is_empty() {
        return Ok(None);
    }
    let entries = tree.split(|byte| *byte == 0).collect::<Vec<_>>();
    if entries.len() != 2 || !entries[1].is_empty() {
        return Err("历史 producer 路径不是唯一普通文件；核对原始 Git 树".into());
    }
    let entry = std::str::from_utf8(entries[0]).map_err(|_| "历史 producer 树项不是 UTF-8")?;
    let (header, name) = entry.split_once('\t').ok_or("历史 producer 树项缺少路径")?;
    let fields = header.split(' ').collect::<Vec<_>>();
    if fields.len() != 3
        || !matches!(fields[0], "100644" | "100755")
        || fields[1] != "blob"
        || name != path
        || fields[2].len() != 40
        || !fields[2]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("历史 producer 路径、mode 或 blob 无效；恢复原始普通文件对象".into());
    }
    // 不只接受 tree 自报的对象名：实际读取对象，缺失或损坏必须失败。
    let _ = staging::git(backend, &["cat-file", "blob", fields[2]])?;
    Ok(Some(fields[2].to_owned()))
}
