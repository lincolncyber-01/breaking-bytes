"""A auditoria deve receber dados reais e falhar se SQLite entrar no executável."""
import json
import unittest
import zlib
from check_binary_sbom import decode, validate


class SbomTests(unittest.TestCase):
    def test_embedded_data_has_one_product_root_and_only_locked_dependencies(self):
        value = {"packages": [{"name": "stories-app", "version": "0.1.0", "root": True}, {"name": "sqlx-postgres", "version": "0.9.0"}]}
        decoded = decode(zlib.compress(json.dumps(value).encode()))
        validate(decoded, {("stories-app", "0.1.0"), ("sqlx-postgres", "0.9.0")})
        with self.assertRaises(ValueError):
            validate(decoded, {("stories-app", "0.1.0")})

    def test_sqlite_or_incomplete_compressed_data_cannot_bypass_the_gate(self):
        value = {"packages": [{"name": "stories-app", "version": "0.1.0", "root": True}, {"name": "libsqlite3-sys", "version": "0.37.0"}]}
        with self.assertRaises(ValueError):
            validate(value, {("stories-app", "0.1.0"), ("libsqlite3-sys", "0.37.0")})
        with self.assertRaises((ValueError, zlib.error)):
            decode(zlib.compress(b'{}')[:-2])
        with self.assertRaises(ValueError):
            decode(zlib.compress(b'x' * (8 * 1024 * 1024 + 2)))


if __name__ == "__main__":
    unittest.main()
