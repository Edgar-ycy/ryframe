"""fresh 目标迁移续作的原生只读证据入口。"""
from pathlib import Path

from devex_clone_capture import read_bound_json, read_json
from devex_clone_model import local_path
from devex_clone_source_proof import bound_file
from devex_clone_target_runtime_evidence import validate_started_generation
from devex_clone_target_evidence import evidence


def _trusted_started(backend: Path, expected: dict, descriptor: dict) -> None:
    started = read_bound_json(bound_file(backend, descriptor), descriptor)
    validate_started_generation(backend, expected, started)


def _trusted_predecessor(backend: Path, expected: dict, descriptor: dict) -> None:
    failure = read_bound_json(bound_file(backend, descriptor), descriptor)
    _trusted_started(backend, expected, failure["started"])


def migration_resume_confirmed(backend: Path, output: Path, initialized: dict) -> bool:
    """畸形或漂移的成功确认保持原失败，消费者不得接受初始化。"""
    try:
        backend = backend.resolve(strict=True)
        output = local_path(backend, str(output))
        if evidence("migration-resume-confirmed", backend, output,
                    initialized=initialized)["confirmed"] is not True:
            return False
        confirmation = read_json(next(output.glob("resume-initialize-*.confirmed.json")))
        _trusted_started(backend, initialized["generation"], confirmation["started"])
        if "predecessor" in confirmation:
            _trusted_predecessor(backend, initialized["generation"], confirmation["predecessor"])
        return evidence("migration-resume-confirmed", backend, output,
                        initialized=initialized)["confirmed"] is True
    except (AttributeError, IndexError, KeyError, OSError, StopIteration, TypeError, ValueError):
        return False
