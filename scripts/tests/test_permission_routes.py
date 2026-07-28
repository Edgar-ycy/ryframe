from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = spec_from_file_location(
    "check_permission_routes", ROOT / "scripts" / "check_permission_routes.py"
)
assert SPEC is not None and SPEC.loader is not None
CHECKER = module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)


class PermissionRouteCheckTest(unittest.TestCase):
    def test_only_fully_allowlisted_route_attributes_skip_permissions(self) -> None:
        self.assertTrue(
            CHECKER.routes_are_authenticated_only(
                "message_handler.rs", ["/", "/unread-count"]
            )
        )
        self.assertFalse(
            CHECKER.routes_are_authenticated_only(
                "message_handler.rs", ["/", "/admin-export"]
            )
        )
        self.assertFalse(CHECKER.routes_are_authenticated_only("message_handler.rs", []))
