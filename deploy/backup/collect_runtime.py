"""Copia bibliotecas dinâmicas realmente usadas e seus metadados, preservando o inventário."""
from pathlib import Path
import os
import shutil
import subprocess

root = Path('/runtime')
root.mkdir(exist_ok=True)
inputs = [Path('/usr/local/bin/python3.15'), Path('/usr/local/pgsql/bin/pg_dump'), Path('/usr/local/pgsql/bin/pg_restore'), Path('/usr/local/pgsql/bin/psql')]
inputs.extend(Path('/usr/local/lib/python3.15/lib-dynload').glob('*.so'))
inputs.extend(Path('/usr/local/aws-cli').rglob('*.so*'))
# As bibliotecas do AWS CLI são privadas da distribuição oficial.
aws_directories = sorted({str(path.parent) for path in inputs if '/aws-cli/' in str(path)})
link_environment = dict(os.environ, LD_LIBRARY_PATH=':'.join(['/usr/local/lib', '/usr/local/pgsql/lib', *aws_directories]))
packages = set()
for binary in inputs:
    result = subprocess.run(['ldd', str(binary)], capture_output=True, text=True, timeout=10, env=link_environment)
    if result.returncode:
        raise RuntimeError("não foi possível inspecionar uma biblioteca do runtime")
    for line in result.stdout.splitlines():
        words = line.split()
        library = words[2] if len(words) > 2 and words[1] == '=>' else words[0] if words else ''
        if not library.startswith('/'):
            if 'not found' in line:
                raise RuntimeError(f'biblioteca do runtime ausente: {binary.name}: {line.strip()}')
            continue
        library = Path(library)
        if str(library).startswith('/usr/local/'):
            continue
        # A base usa /lib como symlink de /usr/lib; copie no destino canônico.
        destination = root / library.resolve().relative_to('/')
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(library.resolve(), destination)
        owners = subprocess.run(['dpkg-query', '-S', str(library.resolve())], capture_output=True, text=True, timeout=10)
        if owners.returncode:
            owners = subprocess.run(['dpkg-query', '-S', str(library)], capture_output=True, text=True, timeout=10)
        for owner in owners.stdout.splitlines():
            package = owner.split(': ', 1)[0]
            if package:
                packages.add(package)
status = root / 'var/lib/dpkg/status.d'
status.mkdir(parents=True, exist_ok=True)
for package in sorted(packages):
    result = subprocess.run(['dpkg-query', '-s', package], capture_output=True, check=True, timeout=10)
    (status / package.split(':')[0]).write_bytes(result.stdout)
    source = Path('/usr/share/doc') / package.split(':')[0] / 'copyright'
    if source.exists():
        destination = root / source.relative_to('/')
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source.resolve(), destination)
