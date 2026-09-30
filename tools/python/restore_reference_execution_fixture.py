"""执行编排的离线运行锁与库存替身；真实合同由对应模块测试覆盖。"""

from contextlib import contextmanager, nullcontext
from pathlib import Path
from unittest.mock import patch

import restore_reference as reference
import restore_reference_execution as execution


def guards(test):
    test.events, test.locked = [], False
    test.before_hook = test.checkpoint_hook = test.image_error = None

    def registration(_backend, path, descriptor):
        document = reference.read_json_document(path)
        if document.value != {"target_plan": descriptor}:
            raise ValueError("登记替身与目标计划不同")
        return document.value, {}, (document,)

    @contextmanager
    def runtime(backend, path, descriptor):
        _, _, documents = registration(backend, path, descriptor)
        test.locked = True
        test.events.append("lock")
        def checkpoint():
            test.assertTrue(test.locked)
            documents[0].assert_unchanged()
            test.events.append("checkpoint")
            if test.checkpoint_hook:
                test.checkpoint_hook()
            return {"registration": reference.document_binding(documents[0]), "target_plan": descriptor}
        try:
            yield checkpoint
            checkpoint()
        finally:
            test.events.append("unlock")
            test.locked = False

    class Images:
        def __init__(self, _backend, plan, _inputs, _tools):
            self.bindings, self.work = {}, Path(plan["work_dir"])

        def control(self):
            test.assertTrue(test.locked)
            return nullcontext()

        def target_environment(self):
            test.assertTrue(test.locked)
            return nullcontext()

        def capture(self, phase, *, after):
            test.assertTrue(test.locked)
            test.events.append(phase)
            if phase == "before" and test.before_hook:
                test.before_hook()
            path = self.work / (phase + "-image.json")
            reference.write_json(path, {"phase": phase, "matches_expected": test.image_error is None})
            self.bindings[phase] = reference.document_binding(reference.read_json_document(path))
            if test.image_error:
                raise ValueError(test.image_error)
            return self.bindings[phase]

    for replacement in (patch.object(execution, "verify_registration", side_effect=registration),
                        patch.object(reference, "registered_stopped_runtime", side_effect=runtime),
                        patch.object(reference, "RestoreImages", Images)):
        replacement.start()
        test.addCleanup(replacement.stop)
