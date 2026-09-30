"""仅为测试解释当前固定核验 argv 与任意业务 stdin，不接受旧核验传输。"""
import subprocess

from restore_reference_io import VerificationStdout

STATEMENTS = {
    "SELECT @@server_uuid, DATABASE();",
    "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;",
}


def mysql_input(command, options):
    if "--execute" in command:
        assert command.count("--execute") == 1
        assert command.index("--execute") == len(command) - 2
        assert options["input"] is None
        assert options["stdin"] == subprocess.DEVNULL
        assert isinstance(options["stdout"], VerificationStdout)
        statement = command[-1]
        assert statement in STATEMENTS
        return statement
    assert not any(part.startswith("--execute=") for part in command)
    assert "stdin" not in options
    statement = options["input"].decode("utf-8")
    assert statement not in STATEMENTS
    return statement


def mysql_output(command, options, raw, *, returncode=0, stderr=b""):
    """固定核验夹具真实写入所传句柄；重定向后 CompletedProcess.stdout 必须为空。"""
    if "--execute" in command:
        assert isinstance(options["stdout"], VerificationStdout)
        options["stdout"].write(raw)
        raw = None
    return subprocess.CompletedProcess(command, returncode, stdout=raw, stderr=stderr)
