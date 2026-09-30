"""XLSX fixture 与恢复验收共用的受限工作簿读取核心。"""

from __future__ import annotations

import io
from pathlib import PurePosixPath
import stat
import xml.etree.ElementTree as ET
import zipfile

SPREADSHEET_NS = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
MAX_ARCHIVE_ENTRIES = 1000
MAX_UNCOMPRESSED_BYTES = 16 * 1024 * 1024


def _invalid_member(entry: zipfile.ZipInfo) -> bool:
    name = entry.filename
    path = PurePosixPath(name)
    unix_mode = entry.external_attr >> 16
    return (
        not name
        or "\\" in name
        or path.is_absolute()
        or ".." in path.parts
        or bool(entry.flag_bits & 0x1)
        or (entry.create_system == 3 and stat.S_ISLNK(unix_mode))
    )


def open_workbook(
    content: bytes,
    *,
    required_members: frozenset[str] = frozenset(),
) -> zipfile.ZipFile:
    """打开已绑定字节，并在返回前验证完整 ZIP 成员集合。"""
    try:
        archive = zipfile.ZipFile(io.BytesIO(content))
    except zipfile.BadZipFile as error:
        raise ValueError("XLSX 不是有效 ZIP") from error
    try:
        entries = archive.infolist()
        names = [entry.filename for entry in entries]
        if (
            len(entries) > MAX_ARCHIVE_ENTRIES
            or len(names) != len(set(names))
            or sum(entry.file_size for entry in entries) > MAX_UNCOMPRESSED_BYTES
            or any(_invalid_member(entry) for entry in entries)
            or not required_members.issubset(names)
        ):
            raise ValueError("XLSX ZIP 结构或解压边界无效")
        return archive
    except Exception:
        archive.close()
        raise


def shared_strings(archive: zipfile.ZipFile) -> list[str]:
    if "xl/sharedStrings.xml" not in archive.namelist():
        return []
    root = ET.fromstring(archive.read("xl/sharedStrings.xml"))
    return ["".join(item.itertext()) for item in root]


def cell_text(cell: ET.Element, strings: list[str]) -> str:
    value = cell.find(f"{{{SPREADSHEET_NS}}}v")
    if cell.get("t") != "s" or value is None:
        return "".join(cell.itertext())
    try:
        index = int(value.text or "")
    except ValueError as error:
        raise ValueError("XLSX shared string 索引无效") from error
    if index < 0 or index >= len(strings):
        raise ValueError("XLSX shared string 索引无效")
    return strings[index]
