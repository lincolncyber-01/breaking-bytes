"""Regressões do portão: uma exceção informativa não pode esconder CVE ou versão nova."""
import copy
import datetime as dt
import unittest
from check_security_exceptions import validate


class ExceptionTests(unittest.TestCase):
    def setUp(self):
        self.entry = {"id": "RUSTSEC-0000-0000", "package": "fixture", "kind": "unmaintained", "expires": "2026-12-17", "versions": ["1.0.0"]}
        self.document = {"advisory": {"id": "RUSTSEC-0000-0000", "package": "fixture", "informational": "unmaintained"}, "versions": {"patched": []}}
        self.today = dt.date(2026, 11, 17)

    def test_maintenance_without_patch_is_limited_to_reviewed_version_and_date(self):
        validate(self.entry, self.document, ["1.0.0"], self.today)
        for versions, today in [(["1.0.1"], self.today), (["1.0.0"], dt.date(2026, 12, 18))]:
            with self.assertRaises(ValueError):
                validate(self.entry, self.document, versions, today)

    def test_cve_unsoundness_patch_or_identity_change_revokes_exception(self):
        for field, value in [("aliases", ["CVE-2099-00001"]), ("informational", "unsound"), ("package", "another")]:
            document = copy.deepcopy(self.document)
            document["advisory"][field] = value
            with self.assertRaises(ValueError):
                validate(self.entry, document, ["1.0.0"], self.today)
        document = copy.deepcopy(self.document)
        document["versions"]["patched"] = [">=1.0.1"]
        with self.assertRaises(ValueError):
            validate(self.entry, document, ["1.0.0"], self.today)


if __name__ == "__main__":
    unittest.main()
