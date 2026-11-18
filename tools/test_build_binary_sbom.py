"""Verifica a ligação entre inventário e compilação real, inclusive backend proibido."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import zlib
from build_binary_sbom import InventoryError, compiled_packages, inventory
from check_binary_sbom import decode, validate


class CompiledInventoryTests(unittest.TestCase):
    def test_real_cargo_artifacts_exclude_inactive_optional_and_keep_activated_backend(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'Cargo.toml').write_text('[workspace]\nmembers=["app","helper","backend"]\nresolver="3"\n')
            for name in ['app', 'helper', 'backend']:
                (root / name / 'src').mkdir(parents=True)
            (root / 'app/Cargo.toml').write_text('[package]\nname="stories-app"\nversion="0.1.0"\nedition="2024"\n[[bin]]\nname="stories-tech"\npath="src/main.rs"\n[dependencies]\nhelper={path="../helper"}\nlibsqlite3-sys={path="../backend",optional=true}\n')
            (root / 'app/src/main.rs').write_text('fn main() { println!("{}", helper::value()); }\n')
            (root / 'helper/Cargo.toml').write_text('[package]\nname="helper"\nversion="0.1.0"\nedition="2024"\n')
            (root / 'helper/src/lib.rs').write_text('pub fn value() -> u32 { 42 }\n')
            (root / 'backend/Cargo.toml').write_text('[package]\nname="libsqlite3-sys"\nversion="0.37.0"\nedition="2024"\n')
            (root / 'backend/src/lib.rs').write_text('pub fn fixture() -> bool { true }\n')
            environment = dict(os.environ, CARGO_TARGET_DIR=str(root / 'target'))
            # O terceiro build conserva o backend no cache, mas não o compila.
            for active in [False, True, False]:
                command = ['cargo', 'build', '--offline', '--manifest-path', str(root / 'app/Cargo.toml'), '--message-format=json']
                if active:
                    command += ['--features', 'libsqlite3-sys']
                result = subprocess.run(command, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=60)
                messages = root / 'build.jsonl'
                messages.write_bytes(result.stdout)
                metadata = json.loads(subprocess.run(['cargo', 'metadata', '--offline', '--format-version', '1', '--manifest-path', str(root / 'app/Cargo.toml')] + (['--features', 'libsqlite3-sys'] if active else []), env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=30).stdout)
                selected, macros = compiled_packages(messages, metadata['resolve']['root'], root / 'target/debug/stories-tech')
                value = inventory(metadata, selected, macros)
                self.assertEqual('libsqlite3-sys' in {p['name'] for p in value['packages']}, active)
                locked = {(p['name'], p['version']) for p in metadata['packages']}
                if active:
                    with self.assertRaisesRegex(ValueError, 'backend_nao_autorizado'):
                        validate(value, locked)
                else:
                    validate(value, locked)
                    binary = root / 'target/debug/stories-tech'
                    section, audited = root / 'dependencies.zlib', root / 'audited'
                    section.write_bytes(zlib.compress(json.dumps(value).encode()))
                    subprocess.run(['objcopy', '--dump-section', '.text=' + str(root / 'text-before'), str(binary), str(root / 'original-copy')], check=True, timeout=10)
                    subprocess.run(['objcopy', '--add-section', '.dep-v0=' + str(section), '--set-section-flags', '.dep-v0=readonly', str(binary), str(audited)], check=True, timeout=10)
                    subprocess.run(['objcopy', '--dump-section', '.dep-v0=' + str(root / 'embedded'), '--dump-section', '.text=' + str(root / 'text-after'), str(audited), str(root / 'inspected-copy')], check=True, timeout=10)
                    self.assertEqual((root / 'text-before').read_bytes(), (root / 'text-after').read_bytes())
                    extracted = decode((root / 'embedded').read_bytes())
                    self.assertEqual(extracted, value)
                    validate(extracted, locked)
                    self.assertEqual(subprocess.check_output([str(audited)], timeout=5).strip(), b'42')

    def test_missing_build_completion_or_artifact_outside_graph_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            messages = root / 'messages.jsonl'
            messages.write_text('{"reason":"build-finished","success":false}\n')
            with self.assertRaises(InventoryError):
                compiled_packages(messages, 'root', root / 'missing')
        metadata = {'packages': [{'id':'root','name':'stories-app','version':'0.1.0'}, {'id':'unexpected','name':'unknown','version':'1.0.0'}], 'resolve':{'root':'root','nodes':[{'id':'root','deps':[]}]}}
        with self.assertRaisesRegex(InventoryError, 'sem_caminho'):
            inventory(metadata, {'root', 'unexpected'}, set())


if __name__ == '__main__':
    unittest.main()
