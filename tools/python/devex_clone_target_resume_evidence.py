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


def migration_resume_state(backend: Path, output: Path, request_descriptor: dict,
                           prepared_files: dict, *,
                           owned_lock_identity: int | None = None) -> dict:
    """锁身份由持锁控制器复核，证据与连续前缀由 Rust 核验。"""
    backend = backend.resolve(strict=True)
    output = local_path(backend, str(output))
    if owned_lock_identity is not None:
        lock = output / "initialize.lock"
        if not lock.is_dir() or lock.stat().st_ino != owned_lock_identity:
            raise ValueError("目标初始化锁身份变化")
    result = evidence("migration-resume-state", backend, output,
                      request_descriptor=request_descriptor, prepared_files=prepared_files,
                      owned_lock=owned_lock_identity is not None)
    if result.get("resumable") and result.get("predecessor") is not None:
        _trusted_predecessor(backend, result["prepared"]["generation"], result["predecessor"])
        if evidence("migration-resume-state", backend, output,
                    request_descriptor=request_descriptor, prepared_files=prepared_files,
                    owned_lock=owned_lock_identity is not None) != result:
            raise ValueError("可信运行代次核验期间迁移续作证据发生变化")
    if owned_lock_identity is not None and (
            not lock.is_dir() or lock.stat().st_ino != owned_lock_identity):
        raise ValueError("目标初始化锁身份变化")
    return result


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
