#!/usr/bin/env python3
"""Executa o Docker com helper temporário e credencial somente em ambiente/memória."""
import contextlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


@contextlib.contextmanager
def configured(environment=None):
    environment = dict(os.environ if environment is None else environment)
    with tempfile.TemporaryDirectory(prefix='stories-registry-') as temporary:
        root = Path(temporary)
        helper = root / 'docker-credential-breakingbytes'
        shutil.copyfile(Path(__file__).with_name('registry_credentials.py'), helper)
        helper.chmod(0o700)
        (root / 'config.json').write_text(json.dumps({'credHelpers': {'ghcr.io': 'breakingbytes'}}))
        yield dict(environment, DOCKER_CONFIG=str(root), PATH=str(root) + os.pathsep + environment.get('PATH', ''))


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in {'push', 'pull', 'manifest'}:
        print('Comando de registro não autorizado.', file=sys.stderr)
        return 1
    try:
        with configured() as environment:
            return subprocess.run(['docker', *sys.argv[1:]], env=environment, timeout=600).returncode
    except (OSError, subprocess.SubprocessError):
        print('Operação de registro falhou ou excedeu o prazo.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
