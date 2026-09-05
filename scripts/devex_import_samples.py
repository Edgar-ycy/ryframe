"""从当前真实模板离线生成全有效导入样本；不连接数据库或产品 API。"""
from __future__ import annotations

import copy
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import sys
import xml.etree.ElementTree as ET
import zipfile

NS = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
MODEL = {"version": 1, "rows_per_file": 1, "username": "dv{namespace}{worker:02x}{cycle:04x}",
         "nickname": "性能导入", "email": "{username}@example.test", "phone": "",
         "department": "current_template_sheet2_A2"}


def digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def canonical(value: dict) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()


def cell_text(cell, strings: list[str]) -> str:
    value = cell.find(f"{{{NS}}}v")
    if cell.get("t") == "s" and value is not None:
        return strings[int(value.text)]
    return "".join(cell.itertext())


def template_archive(path: Path, expected: str):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError("模板必须为显式普通文件且不超过16MiB")
    content = path.read_bytes()
    if digest(content) != expected:
        raise ValueError("模板SHA与固定契约不一致")
    source = zipfile.ZipFile(io.BytesIO(content))
    entries = source.infolist()
    names = [entry.filename for entry in entries]
    if (len(names) != len(set(names)) or len(entries) > 1000
            or sum(entry.file_size for entry in entries) > 16 * 1024 * 1024
            or any(PurePosixPath(name).is_absolute() or ".." in PurePosixPath(name).parts or "\\" in name for name in names)):
        source.close()
        raise ValueError("模板ZIP结构或解压限额不合法")
    return source


def read_model(source):
    strings = []
    if "xl/sharedStrings.xml" in source.namelist():
        strings = ["".join(item.itertext()) for item in ET.fromstring(source.read("xl/sharedStrings.xml"))]
    sheet = ET.fromstring(source.read("xl/worksheets/sheet1.xml"))
    data = sheet.find(f"{{{NS}}}sheetData")
    if data is None or not len(data) or [cell_text(cell, strings) for cell in data[0]] != ["用户名", "昵称", "邮箱", "手机号", "部门完整路径"]:
        raise ValueError("模板表头与当前用户导入契约不一致")
    reference = ET.fromstring(source.read("xl/worksheets/sheet2.xml"))
    first = reference.find(f".//{{{NS}}}c[@r='A2']")
    department = cell_text(first, strings).strip() if first is not None else ""
    if not department:
        raise ValueError("当前模板缺少真实可用部门")
    return sheet, department


def username(namespace: str, worker: int, cycle: int) -> str:
    return f"dv{namespace}{worker:02x}{cycle:04x}"


def workbook(source, original, department: str, user: str, destination: Path) -> None:
    sheet = copy.deepcopy(original)
    data = sheet.find(f"{{{NS}}}sheetData")
    for row in list(data)[1:]:
        data.remove(row)
    row = ET.SubElement(data, f"{{{NS}}}row", {"r": "2"})
    values = [user, MODEL["nickname"], MODEL["email"].format(username=user), "", department]
    for column, value in zip("ABCDE", values):
        cell = ET.SubElement(row, f"{{{NS}}}c", {"r": f"{column}2", "t": "inlineStr"})
        inline = ET.SubElement(cell, f"{{{NS}}}is")
        ET.SubElement(inline, f"{{{NS}}}t").text = value
    dimension = sheet.find(f"{{{NS}}}dimension")
    if dimension is not None:
        dimension.set("ref", "A1:E2")
    with zipfile.ZipFile(destination, "x", compression=zipfile.ZIP_DEFLATED) as target:
        for entry in source.infolist():
            content = ET.tostring(sheet, encoding="utf-8", xml_declaration=True) \
                if entry.filename == "xl/worksheets/sheet1.xml" else source.read(entry)
            target.writestr(copy.copy(entry), content)


def prepare(request: dict) -> dict:
    keys = {"template", "template_sha256", "model_sha256", "directory", "namespace", "concurrency", "cycles"}
    if not isinstance(request, dict) or set(request) != keys:
        raise ValueError("导入样本准备参数不完整")
    if (not re.fullmatch(r"[a-f0-9]{20}", request["namespace"])
            or not re.fullmatch(r"[a-f0-9]{64}", request["template_sha256"])
            or request["model_sha256"] != digest(canonical(MODEL))):
        raise ValueError("导入样本namespace或固定模型SHA无效")
    concurrency, cycles = request["concurrency"], request["cycles"]
    if (type(concurrency) is not int or not 1 <= concurrency <= 100
            or type(cycles) is not int or not 1 <= cycles <= 10000 or concurrency * cycles > 10000):
        raise ValueError("样本维度无效或单次准备超过10000个文件")
    template, directory = Path(request["template"]), Path(request["directory"])
    if not template.is_absolute() or not directory.is_absolute():
        raise ValueError("模板和样本目录必须使用明确绝对路径")
    # 每个测量调用独占目录；拒绝重复命名空间目录及已存在产物，不覆盖前次证据。
    if directory.exists() or directory.is_symlink():
        raise ValueError("样本目录已存在，拒绝复用或覆盖")
    with template_archive(template, request["template_sha256"]) as source:
        sheet, department = read_model(source)
        directory.mkdir()
        records = []
        for worker in range(concurrency):
            for cycle in range(cycles):
                user = username(request["namespace"], worker, cycle)
                filename = f"{worker}-{cycle}.xlsx"
                destination = directory / filename
                workbook(source, sheet, department, user, destination)
                records.append({"sample": f"{worker}:{cycle}", "filename": filename,
                                "username": user, "sha256": digest(destination.read_bytes())})
    return {"format_version": 1, "namespace": request["namespace"], "template_sha256": request["template_sha256"],
            "model": MODEL, "model_sha256": request["model_sha256"], "concurrency": concurrency,
            "cycles": cycles, "files": records}


def main() -> int:
    try:
        raw = sys.stdin.read(32769)
        if len(raw) > 32768:
            raise ValueError("导入样本参数超出限额")
        result = {"ok": True, "manifest": prepare(json.loads(raw))}
    except Exception:
        # 不输出本机模板内容、环境绑定或第三方解析器异常。
        result = {"ok": False, "reason": "import_sample_preparation_failed"}
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
