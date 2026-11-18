#!/usr/bin/env python3
"""Valida provisionamento real do painel no Compose de CI, sem imprimir credenciais."""
import base64
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request


def main():
    root = Path(__file__).resolve().parents[2]
    command = ['docker', 'compose', '--project-name', 'breaking-bytes-monitoring-check',
               '-f', str(root / 'deploy/docker-compose.yml'),
               '-f', str(root / 'deploy/monitoring/compose.yml')]
    password = os.environ['GRAFANA_ADMIN_PASSWORD']
    token = base64.b64encode(('operations:' + password).encode()).decode()
    client = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        subprocess.run(command + ['up', '-d', '--no-deps', 'prometheus', 'grafana'],
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True, timeout=180)
        deadline = time.monotonic() + 60
        while True:
            try:
                request = urllib.request.Request('http://127.0.0.1:3000/api/dashboards/uid/breaking-bytes',
                                                 headers={'Authorization': 'Basic ' + token})
                with client.open(request, timeout=3) as response:
                    result = json.loads(response.read(512 * 1024))
                dashboard = result['dashboard']
                if dashboard['uid'] != 'breaking-bytes' or len(dashboard['panels']) < 6:
                    raise ValueError('painel divergente')
                with client.open('http://127.0.0.1:9090/-/ready', timeout=3) as response:
                    if response.status != 200:
                        raise ValueError('Prometheus indisponível')
                print(json.dumps({'status': 'monitoramento_provisionado', 'panels': len(dashboard['panels'])}))
                return 0
            except (urllib.error.URLError, OSError, ValueError, KeyError):
                if time.monotonic() >= deadline:
                    raise RuntimeError('monitoramento não iniciou dentro do prazo') from None
                time.sleep(1)
    except (subprocess.SubprocessError, RuntimeError, OSError, KeyError):
        print('Falha no contrato real de monitoramento; confira os serviços privados no runner.')
        return 1
    finally:
        subprocess.run(command + ['down', '--volumes'], stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=60)


if __name__ == '__main__':
    raise SystemExit(main())
