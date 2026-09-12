"""从当前真实模板离线生成受控用户导入样本。"""

from __future__ import annotations

import argparse
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ET
import zipfile

from xlsx_workbook import (
    SPREADSHEET_NS as NS,
    cell_text,
    open_workbook,
    shared_strings,
)

HEADERS = ["用户名", "昵称", "邮箱", "手机号", "部门完整路径"]
MODEL = {
    "version": 1,
    "rows_per_file": 1,
    "username": "dv{namespace}{worker:02x}{cycle:04x}",
    "nickname": "性能导入",
    "email": "{username}@example.test",
    "phone": "",
    "department": "current_template_sheet2_A2",
}
MAX_TEMPLATE_BYTES = 16 * 1024 * 1024
MAX_IMPORT_BYTES = 10 * 1024 * 1024
LARGE_PAYLOAD_BYTES = 3 * 1024 * 1024
LARGE_PAYLOAD_NAME = "ryframe-upload-fixture.bin"


def digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def canonical(value: object) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode()


def template_archive(path: Path, expected_sha256: str) -> zipfile.ZipFile:
    resolved = path.resolve(strict=False) if path.is_absolute() else None
    if (
        not path.is_absolute()
        or resolved != path
        or path.is_symlink()
        or not path.is_file()
        or path.stat().st_size > MAX_TEMPLATE_BYTES
        or not re.fullmatch(r"[a-f0-9]{64}", expected_sha256)
    ):
        raise ValueError("模板必须为显式绝对路径普通文件且不超过16MiB")
    content = path.read_bytes()
    if len(content) > MAX_TEMPLATE_BYTES or digest(content) != expected_sha256:
        raise ValueError("模板SHA与固定契约不一致")
    try:
        return open_workbook(content)
    except ValueError as error:
        raise ValueError("模板不是有效XLSX ZIP或结构超出限额") from error


def read_model(source: zipfile.ZipFile) -> tuple[ET.Element, str]:
    strings = shared_strings(source)
    try:
        sheet = ET.fromstring(source.read("xl/worksheets/sheet1.xml"))
        reference = ET.fromstring(source.read("xl/worksheets/sheet2.xml"))
    except (KeyError, ET.ParseError) as error:
        raise ValueError("模板缺少有效工作表") from error
    data = sheet.find(f"{{{NS}}}sheetData")
    if data is None or not len(data):
        raise ValueError("模板缺少用户导入表头")
    if [cell_text(cell, strings) for cell in data[0]] != HEADERS:
        raise ValueError("模板表头与当前用户导入契约不一致")
    first = reference.find(f".//{{{NS}}}c[@r='A2']")
    department = cell_text(first, strings).strip() if first is not None else ""
    if not department:
        raise ValueError("当前模板缺少真实可用部门")
    return sheet, department


def _render_workbook(
    source: zipfile.ZipFile,
    original: ET.Element,
    rows: list[list[str]],
    *,
    large: bool = False,
) -> bytes:
    if not rows or len(rows) > 20_000 or any(len(row) != len(HEADERS) for row in rows):
        raise ValueError("导入样本行结构无效")
    sheet = copy.deepcopy(original)
    data = sheet.find(f"{{{NS}}}sheetData")
    if data is None:
        raise ValueError("模板缺少用户导入数据区")
    for row in list(data)[1:]:
        data.remove(row)
    for number, values in enumerate(rows, 2):
        row = ET.SubElement(data, f"{{{NS}}}row", {"r": str(number)})
        for column, value in zip("ABCDE", values, strict=True):
            cell = ET.SubElement(
                row, f"{{{NS}}}c", {"r": f"{column}{number}", "t": "inlineStr"}
            )
            inline = ET.SubElement(cell, f"{{{NS}}}is")
            ET.SubElement(inline, f"{{{NS}}}t").text = value
    dimension = sheet.find(f"{{{NS}}}dimension")
    if dimension is not None:
        dimension.set("ref", f"A1:E{len(rows) + 1}")
    if large and LARGE_PAYLOAD_NAME in source.namelist():
        raise ValueError("模板占用了大文件样本保留成员")
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_DEFLATED) as target:
        for entry in source.infolist():
            content = (
                ET.tostring(sheet, encoding="utf-8", xml_declaration=True)
                if entry.filename == "xl/worksheets/sheet1.xml"
                else source.read(entry)
            )
            target.writestr(copy.copy(entry), content)
        if large:
            target.writestr(
                LARGE_PAYLOAD_NAME,
                bytes(LARGE_PAYLOAD_BYTES),
                compress_type=zipfile.ZIP_STORED,
            )
    content = buffer.getvalue()
    if len(content) > MAX_IMPORT_BYTES:
        raise ValueError("生成的导入样本超过10MiB产品上限")
    if large and len(content) <= 2 * 1024 * 1024:
        raise ValueError("大文件样本没有达到multipart验证规模")
    return content


def _write_exclusive(output: Path, content: bytes) -> dict[str, object]:
    resolved_parent = (
        output.parent.resolve(strict=False) if output.is_absolute() else None
    )
    if (
        not output.is_absolute()
        or resolved_parent is None
        or resolved_parent / output.name != output
        or output.parent.is_symlink()
        or output.exists()
        or output.is_symlink()
        or not output.parent.is_dir()
    ):
        raise ValueError("输出必须是不存在且父目录已创建的绝对路径")
    temporary = output.parent / f".{output.name}.{os.urandom(12).hex()}.tmp"
    try:
        with temporary.open("xb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.link(temporary, output)
    finally:
        temporary.unlink(missing_ok=True)
    return {"path": str(output), "bytes": len(content), "sha256": digest(content)}


def performance_username(namespace: str, worker: int, cycle: int) -> str:
    return f"dv{namespace}{worker:02x}{cycle:04x}"


def prepare_performance(request: dict[str, object]) -> dict[str, object]:
    keys = {
        "template",
        "template_sha256",
        "model_sha256",
        "directory",
        "namespace",
        "concurrency",
        "cycles",
    }
    if not isinstance(request, dict) or set(request) != keys:
        raise ValueError("导入样本准备参数不完整")
    namespace = request["namespace"]
    template_sha256 = request["template_sha256"]
    if (
        not isinstance(namespace, str)
        or not re.fullmatch(r"[a-f0-9]{20}", namespace)
        or not isinstance(template_sha256, str)
        or not re.fullmatch(r"[a-f0-9]{64}", template_sha256)
        or request["model_sha256"] != digest(canonical(MODEL))
    ):
        raise ValueError("导入样本namespace或固定模型SHA无效")
    concurrency, cycles = request["concurrency"], request["cycles"]
    if (
        type(concurrency) is not int
        or not 1 <= concurrency <= 100
        or type(cycles) is not int
        or not 1 <= cycles <= 10_000
        or concurrency * cycles > 10_000
    ):
        raise ValueError("样本维度无效或单次准备超过10000个文件")
    if not isinstance(request["template"], str) or not isinstance(
        request["directory"], str
    ):
        raise ValueError("模板和样本目录必须使用明确绝对路径")
    template, directory = Path(request["template"]), Path(request["directory"])
    if not template.is_absolute() or not directory.is_absolute():
        raise ValueError("模板和样本目录必须使用明确绝对路径")
    resolved_parent = directory.parent.resolve(strict=False)
    if (
        resolved_parent / directory.name != directory
        or directory.parent.is_symlink()
        or directory.exists()
        or directory.is_symlink()
        or not directory.parent.is_dir()
    ):
        raise ValueError("样本目录已存在或父目录未登记，拒绝复用或覆盖")
    with template_archive(template, template_sha256) as source:
        sheet, department = read_model(source)
        directory.mkdir()
        records: list[dict[str, object]] = []
        for worker in range(concurrency):
            for cycle in range(cycles):
                user = performance_username(namespace, worker, cycle)
                filename = f"{worker}-{cycle}.xlsx"
                row = [user, "性能导入", f"{user}@example.test", "", department]
                receipt = _write_exclusive(
                    directory / filename, _render_workbook(source, sheet, [row])
                )
                records.append(
                    {
                        "sample": f"{worker}:{cycle}",
                        "filename": filename,
                        "username": user,
                        "sha256": receipt["sha256"],
                    }
                )
    return {
        "format_version": 1,
        "namespace": namespace,
        "template_sha256": template_sha256,
        "model": MODEL,
        "model_sha256": request["model_sha256"],
        "concurrency": concurrency,
        "cycles": cycles,
        "files": records,
    }


def prepare_browser(
    template: Path,
    template_sha256: str,
    output: Path,
    username: str,
    *,
    large: bool = False,
) -> dict[str, object]:
    if not re.fullmatch(r"import-[a-z0-9]{8}", username):
        raise ValueError("测试用户名必须使用唯一 import-xxxxxxxx 形式")
    with template_archive(template, template_sha256) as source:
        sheet, department = read_model(source)
        rows = [
            [username, "导入验收", f"{username}@example.test", "", department],
            [username, "重复行", f"{username}@example.test", "", department],
            [
                f"{username}-bad",
                "无效部门",
                f"{username}-bad@example.test",
                "",
                "不存在的隔离部门",
            ],
        ]
        receipt = _write_exclusive(
            output, _render_workbook(source, sheet, rows, large=large)
        )
    return {
        **receipt,
        "template_sha256": template_sha256,
        "username": username,
        "rows": 3,
        "large": large,
    }


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    commands.add_parser("performance", help="从标准输入读取固定性能样本请求")
    browser = commands.add_parser("browser", help="生成真实浏览器导入验收样本")
    browser.add_argument("--template", type=Path, required=True)
    browser.add_argument("--template-sha256", required=True)
    browser.add_argument("--output", type=Path, required=True)
    browser.add_argument("--username", required=True)
    browser.add_argument("--large", action="store_true")
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = _parser().parse_args(argv)
    try:
        if arguments.operation == "performance":
            raw = sys.stdin.read(32_769)
            if len(raw) > 32_768:
                raise ValueError("导入样本参数超出限额")
            result = {"ok": True, "manifest": prepare_performance(json.loads(raw))}
        else:
            result = {
                "ok": True,
                "fixture": prepare_browser(
                    arguments.template,
                    arguments.template_sha256,
                    arguments.output,
                    arguments.username,
                    large=arguments.large,
                ),
            }
    except Exception:
        result = {"ok": False, "reason": "import_fixture_preparation_failed"}
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
