#!/usr/bin/env python3
"""Obtém o relatório de aceite por consulta SQL somente leitura, sem senha em argv."""
import argparse
from pathlib import Path
import sys
from deploy import Deployment
from runtime import OperationError, atomic_json, read_json, timestamp


def main():
    parser = argparse.ArgumentParser(description='Relatório SQL para revisão do lançamento')
    parser.add_argument('--since', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--state', type=Path, default=Path('/var/lib/breaking-bytes'))
    args = parser.parse_args()
    try:
        timestamp(args.since)
        if args.output.exists():
            raise OperationError('relatorio_ja_existe')
        release = read_json(args.state / 'deployment.json')['current']
        deployment = Deployment(Path(__file__).resolve().parents[2], Path(release['snapshot']) / 'config', args.state)
        result = deployment.sql_report(release, args.since)
        atomic_json(args.output, result)
        print('Relatório SQL somente leitura gravado.')
        return 0
    except (OperationError, OSError, KeyError, ValueError):
        print('Não foi possível obter o relatório SQL de aceite.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
