#!/usr/bin/env python3
"""Credential helper Docker somente leitura; nenhum segredo é escrito em arquivo."""
import json
import os
import sys


def main():
    server = sys.stdin.read(257).strip()
    token, actor = os.environ.get('GITHUB_TOKEN', ''), os.environ.get('GITHUB_ACTOR', '')
    if len(sys.argv) != 2 or sys.argv[1] != 'get' or server not in {'ghcr.io', 'https://ghcr.io', 'https://ghcr.io/v1/'}:
        print('Operação de credenciais não autorizada.', file=sys.stderr)
        return 1
    if not 20 <= len(token) <= 8192 or not 1 <= len(actor) <= 100 or any(c in token + actor for c in '\r\n\0'):
        print('Credencial de ambiente ausente ou inválida.', file=sys.stderr)
        return 1
    # O stdout faz parte do protocolo privado do helper, consumido pelo Docker.
    sys.stdout.write(json.dumps({'ServerURL': server, 'Username': actor, 'Secret': token}) + '\n')
    return 0


if __name__ == '__main__':
    sys.exit(main())
