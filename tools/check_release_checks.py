#!/usr/bin/env python3
"""Exige checks concluídos no mesmo SHA antes de publicar uma versão."""
import json
import os
import re
import sys
import urllib.error
import urllib.request

REQUIRED = {'quality', 'domain-coverage', 'render-performance', 'dependencies', 'operations', 'image-scan', 'fuzz'}


def verify(checks):
    latest = {}
    for check in checks:
        name = check.get('name')
        # Os checks exigidos precisam ser emitidos pelo GitHub Actions.
        if name in REQUIRED and check.get('app', {}).get('slug') == 'github-actions':
            if name not in latest or check['id'] > latest[name]['id']:
                latest[name] = check
    return REQUIRED == latest.keys() and all(v.get('status') == 'completed' and v.get('conclusion') == 'success' for v in latest.values())


def main():
    token = os.environ.get('GITHUB_TOKEN', '')
    revision = os.environ.get('GITHUB_SHA', '')
    repository = os.environ.get('GITHUB_REPOSITORY', '')
    if repository != 'lincolncyber-01/breaking-bytes' or not re.fullmatch(r'[a-f0-9]{40}', revision) or not token or any(c in token for c in '\r\n\0'):
        print('Contexto de release inválido.', file=sys.stderr)
        return 1
    try:
        checks = []
        for page in range(1, 6):
            url = f'https://api.github.com/repos/{repository}/commits/{revision}/check-runs?per_page=100&page={page}'
            request = urllib.request.Request(url, headers={'Authorization': 'Bearer ' + token, 'Accept': 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'breaking-bytes-release-gate'})
            with urllib.request.urlopen(request, timeout=10) as response:
                raw = response.read(4 * 1024 * 1024 + 1)
                if len(raw) > 4 * 1024 * 1024:
                    raise ValueError()
            values = json.loads(raw)['check_runs']
            if not isinstance(values, list):
                raise ValueError()
            checks.extend(values)
            if len(values) < 100:
                break
        else:
            raise ValueError()
        if not verify(checks):
            print('Release bloqueado: todos os checks exigidos precisam estar verdes neste SHA.', file=sys.stderr)
            return 1
        print(json.dumps({'status': 'checks_do_sha_aprovados', 'sha': revision, 'checks': sorted(REQUIRED)}))
        return 0
    except (OSError, urllib.error.URLError, ValueError, KeyError, TypeError):
        print('Não foi possível confirmar os checks; release bloqueado.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
