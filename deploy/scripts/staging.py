#!/usr/bin/env python3
"""Inicia staging isolado; publicação exige conta distinta e intenção explícita."""
import argparse
import os
from pathlib import Path
import re
import sys
from acceptance import IMAGE
from deploy import registry_environment
from runtime import OperationError, Runner


def validate(environment, publish):
    if not IMAGE.fullmatch(environment.get('STORIES_IMAGE', '')):
        raise OperationError('imagem_por_digest_obrigatoria')
    if publish:
        stage, production = environment.get('STAGING_IG_USER_ID', ''), environment.get('IG_USER_ID', '')
        if not re.fullmatch(r'\d{1,64}', stage) or not re.fullmatch(r'\d{1,64}', production) or stage == production:
            raise OperationError('staging_exige_conta_distinta')


def isolated_environment(source, publish):
    environment = dict(source)
    validate(environment, publish)
    for base, staging in (
        ('POSTGRES_PASSWORD', 'STAGING_POSTGRES_PASSWORD'),
        ('DATABASE_URL', 'STAGING_DATABASE_URL'),
        ('MIGRATION_DATABASE_URL', 'STAGING_MIGRATION_DATABASE_URL'),
        ('ADMIN_TOKEN', 'STAGING_ADMIN_TOKEN'),
    ):
        if not environment.get(staging):
            raise OperationError('credencial_de_staging_ausente')
        environment[base] = environment[staging]
    environment['STAGING_DRY_RUN'] = 'false' if publish else 'true'
    return environment


def main():
    parser = argparse.ArgumentParser(description='Staging com banco, objetos e conta de teste separados')
    parser.add_argument('command', choices=['prepare', 'start', 'stop'])
    parser.add_argument('--publish', action='store_true')
    args = parser.parse_args()
    try:
        environment = isolated_environment(os.environ, args.publish)
        root = Path(__file__).resolve().parents[2]
        command = ['docker', 'compose', '--project-name', 'breaking-bytes-staging', '-f', str(root / 'deploy/docker-compose.yml'), '-f', str(root / 'deploy/staging-compose.yml')]
        runner = Runner()
        if args.command == 'stop':
            runner.run([*command, 'stop', '--timeout', '30'], environment, timeout=60)
        else:
            with registry_environment(environment) as registry:
                runner.run(['docker', 'pull', environment['STORIES_IMAGE']], registry, timeout=300)
            runner.run([*command, 'config', '--quiet'], environment, timeout=20)
            if args.command == 'prepare':
                for name in ['BOOTSTRAP_DATABASE_URL', 'MIGRATION_DATABASE_PASSWORD', 'RUNTIME_DATABASE_PASSWORD', 'BACKUP_DATABASE_PASSWORD']:
                    if not environment.get('STAGING_' + name):
                        raise OperationError('credencial_de_bootstrap_staging_ausente')
                    environment[name] = environment['STAGING_' + name]
                environment['STORIES_NETWORK'] = 'breaking-bytes-staging_default'
                runner.run([*command, 'up', '-d', '--wait', '--wait-timeout', '90', 'postgres'], environment, timeout=120)
                runner.run(['docker', 'compose', '--project-name', 'breaking-bytes-staging-bootstrap', '-f', str(root / 'deploy/bootstrap-compose.yml'), 'run', '--rm', '--pull', 'never', 'provision'], environment, timeout=180)
            else:
                runner.run([*command, 'up', '-d', '--no-build', '--wait', '--wait-timeout', '120'], environment, timeout=180)
        print('Staging atualizado; volumes preservados.')
        return 0
    except (OperationError, OSError):
        print('Staging recusado; confira isolamento, imagem e credenciais de teste.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
