#!/usr/bin/env python3
"""Generate a deterministic CycloneDX SBOM for backend and frontend inputs."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import subprocess
import sys
import urllib.parse
import uuid
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]


def cargo_components() -> list[dict[str, Any]]:
    completed = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    metadata = json.loads(completed.stdout)
    resolved_ids = {node["id"] for node in metadata["resolve"]["nodes"]}
    components: list[dict[str, Any]] = []
    for package in metadata["packages"]:
        if package["id"] not in resolved_ids:
            continue
        name = package["name"]
        version = package["version"]
        component: dict[str, Any] = {
            "type": "library",
            "bom-ref": f"pkg:cargo/{urllib.parse.quote(name)}@{version}",
            "name": name,
            "version": version,
            "purl": f"pkg:cargo/{urllib.parse.quote(name)}@{version}",
        }
        if package.get("license"):
            component["licenses"] = [{"expression": package["license"]}]
        components.append(component)
    return components


def collect_node_dependencies(node: object, found: set[tuple[str, str]]) -> None:
    if isinstance(node, list):
        for item in node:
            collect_node_dependencies(item, found)
        return
    if not isinstance(node, dict):
        return

    for field in ("dependencies", "devDependencies", "optionalDependencies"):
        dependencies = node.get(field)
        if not isinstance(dependencies, dict):
            continue
        for name, details in dependencies.items():
            if not isinstance(details, dict):
                continue
            version = details.get("version")
            if isinstance(version, str) and version:
                found.add((name, version.split("(", 1)[0]))
            collect_node_dependencies(details, found)


def node_components(path: Path | None) -> list[dict[str, Any]]:
    if path is None:
        return []
    document = json.loads(path.read_text(encoding="utf-8"))
    found: set[tuple[str, str]] = set()
    collect_node_dependencies(document, found)
    components: list[dict[str, Any]] = []
    for name, version in sorted(found):
        if name.startswith("@") and "/" in name:
            namespace, package_name = name.split("/", 1)
            encoded_name = (
                f"{urllib.parse.quote(namespace, safe='')}/"
                f"{urllib.parse.quote(package_name, safe='')}"
            )
        else:
            encoded_name = urllib.parse.quote(name, safe="")
        purl = f"pkg:npm/{encoded_name}@{urllib.parse.quote(version, safe='')}"
        components.append(
            {
                "type": "library",
                "bom-ref": purl,
                "name": name,
                "version": version,
                "purl": purl,
            }
        )
    return components


def timestamp() -> str:
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))
    moment = (
        dt.datetime.fromtimestamp(epoch, tz=dt.timezone.utc)
        if epoch > 0
        else dt.datetime.now(dt.timezone.utc)
    )
    return moment.isoformat(timespec="seconds").replace("+00:00", "Z")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--frontend-dependencies", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    try:
        components = cargo_components() + node_components(args.frontend_dependencies)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"SBOM generation failed: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError):
            detail = (error.stderr or error.stdout or "").strip()
            if detail:
                print(detail, file=sys.stderr)
        return 1
    components.sort(key=lambda item: item["bom-ref"])
    fingerprint = "\n".join(item["bom-ref"] for item in components)
    serial = uuid.uuid5(uuid.NAMESPACE_URL, f"ryframe:{args.version}:{fingerprint}")
    document = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": f"urn:uuid:{serial}",
        "version": 1,
        "metadata": {
            "timestamp": timestamp(),
            "component": {
                "type": "application",
                "bom-ref": f"pkg:github/Edgar-ycy/ryframe@{args.version}",
                "name": "ryframe",
                "version": args.version,
                "purl": f"pkg:github/Edgar-ycy/ryframe@{args.version}",
            },
        },
        "components": components,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"Wrote {len(components)} components to {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
