from __future__ import annotations

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def source(relative_path: str) -> str:
    return (ROOT / relative_path).read_text(encoding="utf-8")


def method_source(relative_path: str, method_name: str) -> str:
    text = source(relative_path)
    match = re.search(rf"\b(?:pub\s+)?(?:async\s+)?fn\s+{method_name}\b", text)
    if match is None:
        raise AssertionError(f"找不到方法: {relative_path}::{method_name}")
    opening = text.find("{", match.end())
    if opening < 0:
        raise AssertionError(f"方法缺少函数体: {relative_path}::{method_name}")

    depth = 0
    for index in range(opening, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return text[match.start() : index + 1]
    raise AssertionError(f"方法函数体未闭合: {relative_path}::{method_name}")


class ReadConsistencyPolicyTest(unittest.TestCase):
    def test_ordinary_queries_use_eventual_consistency(self) -> None:
        cases = {
            "crates/ryframe-service/src/system/login_info_service.rs": (
                "find_by_page",
                "find_for_export",
            ),
            "crates/ryframe-service/src/system/oper_log_service.rs": (
                "find_by_page",
                "find_for_export",
            ),
            "crates/ryframe-service/src/system/notice_service.rs": ("find_by_page",),
            "crates/ryframe-service/src/system/export_service.rs": ("list_for_requester",),
        }
        for relative_path, methods in cases.items():
            for method in methods:
                with self.subTest(path=relative_path, method=method):
                    body = method_source(relative_path, method)
                    self.assertIn("ReadConsistency::Eventual", body)
                    self.assertNotIn("ReadConsistency::Strong", body)

    def test_security_and_write_after_read_queries_use_strong_consistency(self) -> None:
        cases = {
            "crates/ryframe-service/src/system/file_service.rs": ("download",),
            "crates/ryframe-service/src/system/export_service.rs": (
                "find_for_requester",
                "cancel_for_requester",
                "download_location_for_requester",
            ),
        }
        for relative_path, methods in cases.items():
            for method in methods:
                with self.subTest(path=relative_path, method=method):
                    body = method_source(relative_path, method)
                    self.assertIn("ReadConsistency::Strong", body)

    def test_cache_hits_do_not_select_a_database_node(self) -> None:
        cases = (
            (
                "crates/ryframe-service/src/system/config_service.rs",
                "find_by_key_in_tenant",
                ".read_namespace_value(",
            ),
            (
                "crates/ryframe-service/src/system/dict_service.rs",
                "find_data_by_type",
                ".get(&dict_cache_key",
            ),
        )
        for relative_path, method, cache_read in cases:
            with self.subTest(path=relative_path, method=method):
                body = method_source(relative_path, method)
                self.assertLess(body.index(cache_read), body.index(".select_read("))

    def test_api_handlers_do_not_route_database_connections(self) -> None:
        for path in sorted((ROOT / "crates/ryframe-api/src").rglob("*.rs")):
            text = path.read_text(encoding="utf-8")
            with self.subTest(path=path.relative_to(ROOT)):
                self.assertNotIn("DatabaseCluster", text)
                self.assertNotIn("select_read(ReadConsistency", text)

    def test_read_router_does_not_replay_failed_sql(self) -> None:
        body = method_source("crates/ryframe-db/src/cluster.rs", "select_read")
        for forbidden in (".await", ".execute(", ".query_one(", ".query_all("):
            self.assertNotIn(forbidden, body)

    def test_worker_topology_is_explicitly_primary_only(self) -> None:
        worker = source("crates/ryframe/src/bin/ryframe_worker.rs")
        self.assertIn("DatabaseCluster::single(primary)", worker)


if __name__ == "__main__":
    unittest.main()
