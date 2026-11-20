#!/usr/bin/env python3
"""Exporta o grafo compilado pelo Cargo estável no formato interoperável .dep-v0."""
import json
from pathlib import Path
import sys
import tomllib
import zlib
from check_binary_sbom import validate


class InventoryError(Exception):
    """Erro controlado: mensagens externas e caminhos não entram nos logs."""


def read_json(path, maximum=32 * 1024 * 1024):
    if path.stat().st_size > maximum:
        raise InventoryError('entrada_acima_do_limite')
    return json.loads(path.read_text())


def compiled_packages(path, root, executable):
    if path.stat().st_size > 64 * 1024 * 1024:
        raise InventoryError('compilacao_acima_do_limite')
    selected, macros = set(), set()
    finished = False
    root_binary = False
    with path.open() as stream:
        for line in stream:
            if len(line) > 1024 * 1024 or finished:
                raise InventoryError('mensagem_de_compilacao_invalida')
            item = json.loads(line)
            reason = item.get('reason')
            if reason == 'compiler-artifact':
                identifier = item['package_id']
                selected.add(identifier)
                kinds = item['target']['kind']
                if 'proc-macro' in kinds:
                    macros.add(identifier)
                if identifier == root and 'bin' in kinds and item['target']['name'] == 'stories-tech':
                    binary = item.get('executable')
                    if not binary or Path(binary).resolve() != executable.resolve() or not executable.is_file():
                        raise InventoryError('executavel_divergente')
                    root_binary = True
            elif reason == 'build-finished':
                if item.get('success') is not True:
                    raise InventoryError('compilacao_nao_concluida')
                finished = True
            elif reason not in {'compiler-message', 'build-script-executed'}:
                raise InventoryError('mensagem_do_cargo_desconhecida')
    if not finished or not root_binary or not 1 <= len(selected) <= 1000:
        raise InventoryError('prova_de_compilacao_incompleta')
    return selected, macros


def inventory(metadata, selected, macros):
    root = metadata['resolve']['root']
    packages = {p['id']: p for p in metadata['packages']}
    nodes = {p['id']: p for p in metadata['resolve']['nodes']}
    if root not in selected or packages[root]['name'] != 'stories-app' or not selected <= packages.keys():
        raise InventoryError('pacotes_divergentes')
    # Cargo metadata pode incluir opcionais inativos. A inclusão no inventário
    # exige compiler-artifact real; nenhum nome de pacote é removido por política.
    levels = {root: 2}
    pending = [root]
    while pending:
        parent = pending.pop()
        for dependency in nodes[parent]['deps']:
            child = dependency['pkg']
            if child not in selected:
                continue
            kinds = dependency['dep_kinds']
            strength = max((2 if k['kind'] is None else 1 if k['kind'] == 'build' else 0 for k in kinds), default=0)
            strength = min(strength, levels[parent], 1 if child in macros else 2)
            if strength > levels.get(child, 0):
                levels[child] = strength
                pending.append(child)
    if set(levels) != selected:
        raise InventoryError('artefato_compilado_sem_caminho_no_grafo')
    ordered = sorted(selected, key=lambda key: (packages[key]['name'], packages[key]['version'], key))
    indices = {identifier: index for index, identifier in enumerate(ordered)}
    result = []
    for identifier in ordered:
        package = packages[identifier]
        source = package.get('source')
        if source is None:
            origin = 'local'
        elif source in {'registry+https://github.com/rust-lang/crates.io-index', 'sparse+https://index.crates.io/'}:
            origin = 'crates.io'
        elif source.startswith('git+'):
            origin = 'git'
        else:
            origin = 'registry'
        edges = sorted({indices[d['pkg']] for d in nodes[identifier]['deps']
                        if d['pkg'] in selected and any(k['kind'] != 'dev' for k in d['dep_kinds'])})
        item = {'name': package['name'], 'version': package['version'], 'source': origin,
                'kind': 'runtime' if levels[identifier] == 2 else 'build', 'dependencies': edges}
        if identifier == root:
            item['root'] = True
        result.append(item)
    # Formato 1: grafo de cargo metadata, com proc macros classificados como build.
    # A procedência adicional dos compiler-artifacts é documentada no ADR-018.
    return {'format': 1, 'packages': result}


def main():
    try:
        if len(sys.argv) != 7:
            raise InventoryError('argumentos_invalidos')
        metadata_path, messages, executable, lock_path, output, compressed = map(Path, sys.argv[1:])
        metadata = read_json(metadata_path)
        selected, macros = compiled_packages(messages, metadata['resolve']['root'], executable)
        value = inventory(metadata, selected, macros)
        lock = tomllib.loads(lock_path.read_text())
        raw = json.dumps(value, separators=(',', ':'), sort_keys=True).encode()
        output.write_bytes(raw + b'\n')
        compressed.write_bytes(zlib.compress(raw, 7))
        validate(value, {(p['name'], p['version']) for p in lock['package']})
        print(json.dumps({'status': 'inventario_compilado_verificado', 'packages': len(selected), 'format': 1}))
        return 0
    except (InventoryError, OSError, ValueError, KeyError, TypeError):
        print('Falha ao exportar o inventário de compilação; o artefato não pode ser entregue.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
