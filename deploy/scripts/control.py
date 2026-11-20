#!/usr/bin/env python3
"""Opera a pausa sem colocar o token em argumentos ou arquivos."""
import argparse
import json
import sys
from runtime import Admin, OperationError


def main():
    parser = argparse.ArgumentParser(description='Administração privada do pipeline')
    parser.add_argument('command', choices=['status', 'stats', 'pause', 'resume'])
    parser.add_argument('--port', type=int, default=8080)
    args = parser.parse_args()
    try:
        client = Admin(port=args.port)
        if args.command in {'pause', 'resume'}:
            client.pause(args.command == 'pause')
            print(json.dumps({'paused': args.command == 'pause'}))
        else:
            print(json.dumps(client.request('/admin/' + args.command), ensure_ascii=False))
        return 0
    except OperationError:
        print('Operação administrativa recusada ou indisponível.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
