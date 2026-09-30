"""正式恢复复用四目标库存及对象观察，固定完整前像、后像和保留元数据。"""

from contextlib import contextmanager
import copy
from dataclasses import asdict
import json
from pathlib import Path

from devex_clone_capture import CaptureReader, write_json
from devex_clone_factory_context import inventory_history, target_history
from process_environment import Environments, configured
from devex_clone_inventory import capture_side_inventory
from devex_clone_live_object import observe_target_object, read_observation, verify_target_heads
from devex_clone_model import exact, local_path, schema_fingerprints
from devex_clone_run import target_storage_control
from devex_clone_seed_arm import verify_published_arm_input
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import execution_backend
from restore_reference_backup import bound_document, document_binding
from restore_reference_io import ExternalTools
from restore_reference_plan import BUCKETS, EXCLUDED_TABLES, plan_hash
from restore_runtime_evidence import read_json_document
from source_fingerprints import artifact_sources


def expected_images(plan: dict, manifest: dict, initialized: dict) -> tuple[dict, dict]:
    before = {"databases": initialized["inventory"]["observations"],
              "objects": initialized["objects"], "redis": initialized["redis"]}
    after = copy.deepcopy(before)
    fingerprints = schema_fingerprints(manifest)
    for database in manifest["databases"]:
        target = next(item for item in plan["target"]["databases"] if item["key"] == database["key"])
        image = after["databases"][target["database"]]
        restored = {row["table"]: {field: row[field] for field in ("rows", "sha256")} for row in database["tables"]}
        schema = {**fingerprints, **({"control_schema_fingerprint": None} if target["kind"] != "combined" else {})}
        if (set(restored) != set(image["all_tables"]) - EXCLUDED_TABLES
                or image["schema_sha256"] != plan_hash(schema)):
            raise ValueError("正式备份必须覆盖目标全部非保留表且 schema 完全一致")
        for field in ("tables", "preserved"):
            image[field].update({key: value for key, value in restored.items() if key in image[field]})
    for objects in manifest["objects"]:
        keys = [plan["target"]["scope_id"] + "/" + item["key"].removeprefix(plan["source"]["scope_id"] + "/")
                for item in objects["entries"]]
        after["objects"][objects["bucket"]]["keys"] = sorted([plan["target"]["scope_id"] + "/.ryframe-owner", *keys])
    return before, after


class RestoreImages:
    def __init__(self, backend: Path, plan: dict, inputs, tools: ExternalTools):
        self.backend, self.plan, self.inputs, self.tools = backend, plan, inputs, tools
        self.arm = verify_published_arm_input(backend, Path(inputs.target.value["arm_input"]["path"]), plan["target_side"])
        if self.arm["binding"] != inputs.target.value["arm_input"]:
            raise ValueError("恢复前后像未绑定同一 arm")
        self.environment_document = bound_document(backend, inputs.target.value["fresh_target"]["environment"])
        exact(self.environment_document.value, {"environment"})
        self.environment = Environments({}, configured(self.environment_document.value["environment"]))
        self.before, self.after = expected_images(plan, inputs.manifest.value, self.arm["initialized"])
        self.bindings = {}

    @contextmanager
    def control(self):
        execution, _ = execution_backend(self.backend, self.arm["target"])
        with artifact_sources(execution, self.arm["request"]["build_bridges"]), target_storage_control(
                self.backend, Path(self.plan["work_dir"]), self.arm["manifest"]):
            yield self

    def target_environment(self):
        self.environment_document.assert_unchanged()
        return self.environment.use("target")

    def _objects(self, resources, tools, output: Path, *, after: bool) -> tuple[dict, list]:
        if not after:
            return resources.objects(initialized=True), []
        reader = CaptureReader(tools, "target", output)
        owners = {item["bucket"]: {field: item[field] for field in ("bytes", "sha256")}
                  for item in reader.owners("owners-before")}
        result = {}
        for bucket in sorted(BUCKETS):
            keys = sorted(resources.object_keys(bucket))
            result[bucket] = {"keys": keys, "owner": owners[bucket]}
            if result[bucket] != self.after["objects"][bucket]:
                raise ValueError("恢复后五桶完整范围或 ownership 不符")
        evidence, items, observations = [], [], {}
        for objects in self.inputs.manifest.value["objects"]:
            for index, entry in enumerate(objects["entries"]):
                key = self.plan["target"]["scope_id"] + "/" + entry["key"].removeprefix(self.plan["source"]["scope_id"] + "/")
                directory = output / f"object-{objects['bucket']}-{index}"
                observed = observe_target_object(self.backend, tools, objects["bucket"], key, directory,
                    expected={field: entry[field] for field in ("bytes", "sha256")}, max_bytes=entry["bytes"])
                receipt = read_json_document(directory / "observation.json")
                if (observed.capture_directory is None or receipt.value["status"] != "object_present"
                        or read_observation(directory, expected_sha256=receipt.sha256) != receipt.value):
                    raise ValueError("恢复后对象未取得完整字节证明")
                receipt.assert_unchanged()
                evidence.append(document_binding(receipt))
                capture = read_json_document(observed.capture_directory / "capture.json")
                items.append({"bucket": objects["bucket"], "target_key": key, "artifact": entry,
                              "metadata": capture.value["metadata"]})
                observations[objects["bucket"], key] = observed
        verify_target_heads(self.backend, tools, items, observations, output / "object-heads")
        evidence.append(document_binding(read_json_document(output / "object-heads/verified.json")))
        repeated = {item["bucket"]: {field: item[field] for field in ("bytes", "sha256")}
                    for item in reader.owners("owners-after")}
        if repeated != owners or any(sorted(resources.object_keys(bucket)) != result[bucket]["keys"] for bucket in BUCKETS):
            raise ValueError("恢复后对象采集期间五桶范围或 ownership 发生变化")
        return result, evidence

    def capture(self, phase: str, *, after: bool) -> dict:
        output = local_path(self.backend, str(Path(self.plan["work_dir"]) / f"restore-{self.plan['target_side']}-{phase}"), new=True)
        output.mkdir()
        try:
            with self.target_environment() as environment:
                initial, request, resources = target_history(self.backend, self.inputs.target.value["fresh_target"]["initialized"],
                    output, self.tools.run, storage_run=Path(self.arm["target_storage_run"]["path"]))
                if initial != self.arm["initialized"] or request != self.arm["target"]:
                    raise ValueError("恢复库存实际目标代次与已绑定 arm 不同")
                side = {**request["target"], **{key: resources.selected[key] for key in ("runtime_dir", "api_url")}}
                tools = ExternalTools({"target": side, "tools": request["tools"]}, output, self.tools.run)
                captured = capture_side_inventory(resources.execution_backend, "target", tools,
                    bound_file(self.backend, request["maintenance_build"]), output / "inventory",
                    environment=environment, evidence_root=self.backend)
                databases = {item.resource["database"]: json.loads(json.dumps(asdict(item))) for item in captured.observations}
                objects, object_evidence = self._objects(resources, tools, output, after=after)
                redis = resources.redis_state(initialized=True, sentinel=True)
                target_history(self.backend, self.inputs.target.value["fresh_target"]["initialized"],
                    output, self.tools.run, storage_run=Path(self.arm["target_storage_run"]["path"]))
            self.inputs.assert_unchanged()
            self.environment_document.assert_unchanged()
            bound_document(self.backend, captured.receipt_file).assert_unchanged()
            inventory_history(self.backend, output, {"receipt": captured.receipt_file, "observations": databases},
                              request, inventory_directory=output / "inventory")
            for descriptor in object_evidence:
                bound_document(self.backend, descriptor).assert_unchanged()
            actual = {"databases": databases, "objects": objects, "redis": redis}
            expected = self.after if after else self.before
            value = {"format_version": 1, "kind": "restore-reference-resource-image", "phase": phase,
                     "target_plan": document_binding(self.inputs.target), "inventory": captured.receipt_file,
                     "object_observations": object_evidence,
                     "image": actual, "expected_sha256": plan_hash(expected), "matches_expected": actual == expected}
            write_json(output / "image.json", value)
            self.bindings[phase] = document_binding(read_json_document(output / "image.json"))
            if actual != expected:
                raise ValueError("恢复完整资源像与预期不同；保留表、业务表和 ownership 均不得省略")
            return self.bindings[phase]
        except BaseException as error:
            try:
                path = output / "failure.json"
                write_json(path, {"phase": phase, "error_type": type(error).__name__})
                self.bindings[phase + "-failure"] = document_binding(read_json_document(path))
            except BaseException as failure:
                error.add_note("资源像失败收据未能保存：" + type(failure).__name__)
            raise
