#!/usr/bin/env python3
"""Exercita rollout real em Compose descartável e rede interna, sem publicar."""
import argparse
import datetime as dt
import hashlib
from http.client import HTTPResponse
import io
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'deploy/scripts'))
from deploy import Deployment
from runtime import Admin, OperationError, Runner, atomic_json, read_json


def require(condition, code):
    if not condition:
        raise OperationError(code)


class LocalImages(Runner):
    """Só o transporte do registry é substituído por imagens locais já construídas."""
    def __init__(self, images, project, private_values):
        self.images = images
        self.project = project
        self.private_values = private_values
        self.commands = []
        self.failed_command = None
        self.shutdown_codes = []
        self.sanitized_error = None

    def run(self, arguments, environment=None, timeout=120, capture=False, input_data=None):
        require(not any(secret in argument for secret in self.private_values for argument in arguments),
                'segredo_encontrado_em_argumento')
        arguments = [self.images.get(argument, argument) for argument in arguments]
        environment = dict(environment) if environment is not None else None
        if environment is not None and environment.get('STORIES_IMAGE') in self.images:
            environment['STORIES_IMAGE'] = self.images[environment['STORIES_IMAGE']]
        if '--project-name' in arguments:
            index = arguments.index('--project-name') + 1
            if arguments[index] == 'breaking-bytes':
                arguments[index] = self.project
            require(arguments[index] in {self.project, self.project + '-bootstrap'},
                    'projeto_fora_do_fixture')
        self.commands.append(arguments)
        if arguments[:2] == ['docker', 'pull']:
            require(arguments[-1] in self.images.values(), 'pull_fora_do_fixture')
            # O pull autenticado real permanece no aceite externo do GHCR.
            super().run(['docker', 'image', 'inspect', '--format', '{{.Id}}', arguments[-1]],
                        environment, timeout=15, capture=True)
            return ''
        try:
            with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
                process = subprocess.run(arguments, env=environment, input=input_data,
                                         stdin=subprocess.DEVNULL if input_data is None else None,
                                         stdout=stdout, stderr=stderr, timeout=timeout)
                if process.returncode:
                    stderr.seek(0)
                    message = stderr.read(65536).decode('utf-8', errors='replace')
                    for value in self.private_values:
                        message = message.replace(value, '[redigido]')
                    self.sanitized_error = message
                    raise OperationError('comando_operacional_falhou')
                stdout.seek(0)
                raw = stdout.read(65537) if capture else b''
                require(len(raw) <= 65536, 'resposta_de_comando_excessiva')
                result = raw.decode()
            if arguments[:2] == ['docker', 'compose'] and 'stop' in arguments and arguments[-1] == 'app':
                prefix = arguments[:arguments.index('stop')]
                container = self.run([*prefix, 'ps', '-a', '-q', 'app'], environment, capture=True).strip()
                if container:
                    code = self.run(['docker', 'inspect', '--format', '{{.State.ExitCode}}', container], environment, capture=True)
                    self.shutdown_codes.append(int(code))
            return result
        except (OperationError, OSError, subprocess.SubprocessError, UnicodeError):
            self.failed_command = arguments
            raise OperationError('comando_operacional_falhou') from None

class BoundedAdmin(Admin):
    """HTTP real na rede interna, sem publicar porta nem passar token em argv."""
    def __init__(self, token, runner, environment, command):
        super().__init__(token)
        self.runner, self.environment, self.command = runner, environment, command
        self.probes = []

    def request(self, path, payload=None):
        require(path in {'/health/ready', '/admin/status', '/admin/stats', '/admin/pause'},
                'rota_do_probe_invalida')
        body = json.dumps(payload) if payload is not None else ''
        method = 'PUT' if payload is not None else 'GET'
        # Bash e timeout pertencem à imagem PostgreSQL. printf é builtin: o token
        # permanece no ambiente, sem aparecer no argv de outro executável.
        script = ('exec 3<>/dev/tcp/app/8080; '
                  'printf "%s %s HTTP/1.1\\r\\nHost: app\\r\\nConnection: close\\r\\nAuthorization: Bearer %s\\r\\nContent-Type: application/json\\r\\nContent-Length: %s\\r\\n\\r\\n%s" '
                  '"$PROBE_METHOD" "$PROBE_PATH" "$ADMIN_TOKEN" "${#PROBE_BODY}" "$PROBE_BODY" >&3; '
                  'cat <&3')
        try:
            probe = {'path': path}
            self.probes.append(probe)
            environment = dict(self.environment, ADMIN_TOKEN=self.token, PROBE_METHOD=method,
                               PROBE_PATH=path, PROBE_BODY=body)
            raw = self.runner.run([*self.command, 'exec', '-T',
                                   '--user', '65534:65534', '-e', 'ADMIN_TOKEN', '-e', 'PROBE_METHOD',
                                   '-e', 'PROBE_PATH', '-e', 'PROBE_BODY', 'postgres',
                                   'timeout', '5', 'bash', '-c', script], environment, timeout=10, capture=True)
            probe['response_bytes'] = len(raw.encode())

            class BufferedSocket:
                def makefile(self, *_):
                    return io.BytesIO(raw.encode())

            response = HTTPResponse(BufferedSocket())
            response.begin()
            probe['status'] = response.status
            require(response.status == 200, 'api_administrativa_indisponivel')
            value = json.loads(response.read(65536))
            require(isinstance(value, dict), 'resposta_administrativa_invalida')
            probe['fields'] = {key: value[key] for key in ['status', 'dry_run', 'daily_cap', 'paused'] if key in value}
            return value
        except (OperationError, OSError, ValueError):
            raise OperationError('api_administrativa_indisponivel') from None

    def wait_ready(self, dry_run, cap, timeout=15):
        return super().wait_ready(dry_run, cap, timeout)


def source_fixture(root, postgres_image):
    source = root / 'source'
    (source / 'deploy/sql').mkdir(parents=True)
    shutil.copyfile(ROOT / 'deploy/sql/acceptance.sql', source / 'deploy/sql/acceptance.sql')
    content = (ROOT / 'deploy/docker-compose.yml').read_text()
    require(content.count('127.0.0.1:8080:8080') == 1 and '\nnetworks:' not in content,
            'template_compose_alterado')
    content = content.replace('    ports:\n      - 127.0.0.1:8080:8080\n', '')
    if postgres_image:
        content, changes = re.subn(r'(?m)^    image: pgvector/\S+$',
                                   '    image: ' + postgres_image, content)
        require(changes == 1, 'imagem_postgres_nao_identificada')
        content = content.replace('      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:?Defina POSTGRES_PASSWORD no ambiente}',
                                  '      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:?Defina POSTGRES_PASSWORD no ambiente}\n'
                                  '      POSTGRES_HOST_AUTH_METHOD: scram-sha-256\n'
                                  '      POSTGRES_INITDB_ARGS: --auth-host=scram-sha-256')
    content += '\nnetworks:\n  default:\n    internal: true\n'
    (source / 'deploy/docker-compose.yml').write_text(content)
    shutil.copytree(ROOT / 'config', source / 'config')
    return source, content


def run(image, postgres_image, report_path):
    require(re.fullmatch(r'[a-z0-9][a-z0-9._/:@-]{0,250}', image) is not None,
            'imagem_local_invalida')
    if postgres_image:
        require(re.fullmatch(r'[a-z0-9][a-z0-9._/:@-]{0,250}', postgres_image) is not None,
                'imagem_postgres_invalida')
    report = {'schema': 1, 'driver_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'scope': 'Compose e Postgres reais, imagens locais; sem GHCR/Meta/R2 reais',
              'started_at': dt.datetime.now(dt.timezone.utc).isoformat(), 'passed': False,
              'checks': [], 'failure_code': None}
    started = time.monotonic()
    stage = 'preparacao'
    runner, command, environment = None, None, None
    with tempfile.TemporaryDirectory(prefix='bb-rollout-') as temporary:
        root = Path(temporary)
        project = 'bb-rollout-' + uuid.uuid4().hex[:16]
        second_image = project + ':second'
        private = {name: secrets.token_hex(32) for name in [
            'POSTGRES_PASSWORD', 'MIGRATION_DATABASE_PASSWORD', 'RUNTIME_DATABASE_PASSWORD',
            'BACKUP_DATABASE_PASSWORD', 'ADMIN_TOKEN', 'ROLLOUT_WRONG_ADMIN_TOKEN',
            'ROLLOUT_WRONG_DATABASE_PASSWORD']}
        docker_config = root / 'docker-config'
        docker_config.mkdir(mode=0o700)
        (docker_config / 'config.json').write_text('{"auths":{}}')
        environment = dict(private, PATH=os.environ.get('PATH', ''),
                           DOCKER_HOST=os.environ.get('DOCKER_HOST', 'unix:///var/run/docker.sock'),
                           DOCKER_CONFIG=str(docker_config), STORIES_IMAGE=image,
                           DATABASE_URL='postgres://stories_runtime:' + private['RUNTIME_DATABASE_PASSWORD'] + '@postgres/stories',
                           MIGRATION_DATABASE_URL='postgres://stories_owner:' + private['MIGRATION_DATABASE_PASSWORD'] + '@postgres/stories',
                           BOOTSTRAP_DATABASE_URL='postgres://postgres:' + private['POSTGRES_PASSWORD'] + '@postgres/stories',
                           ROLLOUT_BAD_MIGRATION_DATABASE_URL='postgres://nonexistent_role:' + private['RUNTIME_DATABASE_PASSWORD'] + '@postgres/stories',
                           LLM_API_KEY='fixture-only-not-a-provider-key',
                           TELEGRAM_BOT_TOKEN='123456789:fixture-only-not-a-bot-token',
                           TELEGRAM_CHAT_ID='123456789', STORIES_NETWORK=project + '_default')
        require(environment['DOCKER_HOST'].startswith('unix://'), 'daemon_remoto_recusado')
        source, original = source_fixture(root, postgres_image)
        compose_path = source / 'deploy/docker-compose.yml'
        command = ['docker', 'compose', '--project-name', project, '-f', str(compose_path)]
        runner = LocalImages({}, project, list(private.values()))
        admin = BoundedAdmin(private['ADMIN_TOKEN'], runner, environment, command)
        deployment = Deployment(source, source / 'config', root / 'state', environment, runner, admin)

        def check(name):
            report['checks'].append(name)
            print('PASS ' + name, flush=True)

        def sentinel(release):
            value = deployment.postgres(release, ['-c', "SELECT count(*) FROM audit_log WHERE step='rollout_fixture'"], read_only=True)
            require(value.strip() == '1', 'dados_perdidos_na_reversao')

        def active(release):
            state = read_json(deployment.record)
            require(state['current']['image'] == release['image'], 'imagem_ativa_divergente')
            require(state['current']['container_id'] == deployment.container(state['current'], deployment.environment_for(state['current'])),
                    'identidade_do_container_divergente')
            require(admin.request('/admin/status')['paused'] is True, 'rollback_nao_conservou_pausa')
            sentinel(state['current'])

        try:
            stage = 'imagens_locais'
            report['compose_version'] = runner.run(['docker', 'compose', 'version'], environment, capture=True).strip()
            raw = runner.run(['docker', 'image', 'inspect', '--format', '{{.Id}} {{.Architecture}} {{index .Config.Labels "org.opencontainers.image.revision"}}', image], environment, capture=True)
            image_id, architecture, revision = raw.split()
            require(architecture == 'amd64' and re.fullmatch(r'[a-f0-9]{40}', revision),
                    'imagem_local_sem_revisao_amd64')
            require(re.fullmatch(r'sha256:[a-f0-9]{64}', image_id), 'id_da_imagem_invalido')
            dockerfile = ('FROM ' + image + '\nLABEL org.breaking-bytes.rollout-fixture=second\n').encode()
            runner.run(['docker', 'build', '--network', 'none', '--pull=false', '-t', second_image, '-'],
                       environment, timeout=120, input_data=dockerfile)
            second_id = runner.run(['docker', 'image', 'inspect', '--format', '{{.Id}}', second_image], environment, capture=True).strip()
            require(second_id != image_id, 'fixture_exige_imagens_distintas')
            first = 'ghcr.io/lincolncyber-01/breaking-bytes@' + image_id
            second = 'ghcr.io/lincolncyber-01/breaking-bytes@' + second_id
            runner.images.update({first: image, second: second_image})
            report.update(revision=revision, image_id=image_id, second_image_id=second_id)

            stage = 'bootstrap_real'
            runner.run([*command, 'up', '-d', '--wait', '--wait-timeout', '90', 'postgres'], environment, timeout=120)
            runner.run(['docker', 'compose', '--project-name', project + '-bootstrap', '-f', str(ROOT / 'deploy/bootstrap-compose.yml'),
                        'run', '--rm', '--pull', 'never', 'provision'], environment, timeout=60)
            check('bootstrap_com_papeis_separados')

            stage = 'primeiro_rollout'
            initial = deployment.apply('dry_run', first, resume=True)
            status = admin.request('/admin/status')
            require(status['dry_run'] is True and status['paused'] is False and status['daily_cap'] == 10,
                    'modo_teto_ou_retomada_divergente')
            admin.pause(True)
            deployment.postgres(initial, ['-c', "INSERT INTO audit_log(step,detail) VALUES('rollout_fixture','{\"preserve\":true}')"])
            require(deployment.postgres(initial, ['-c', "SELECT current_user"], read_only=True).strip() == 'stories_runtime',
                    'consulta_usou_papel_administrativo')
            correct_database = deployment.environment['DATABASE_URL']
            deployment.environment['DATABASE_URL'] = 'postgres://stories_runtime:' + private['ROLLOUT_WRONG_DATABASE_PASSWORD'] + '@postgres/stories'
            try:
                deployment.postgres(initial, ['-c', 'SELECT 1'], read_only=True)
            except OperationError:
                pass
            else:
                raise OperationError('senha_invalida_foi_aceita')
            finally:
                deployment.environment['DATABASE_URL'] = correct_database
            runner.failed_command = None
            runner.sanitized_error = None
            hardening = json.loads(runner.run(['docker', 'inspect', '--format', '{{json .HostConfig}}', initial['container_id']], environment, capture=True))
            require(hardening['ReadonlyRootfs'] is True and 'ALL' in hardening['CapDrop'], 'container_sem_hardening')
            user = runner.run(['docker', 'inspect', '--format', '{{.Config.User}}', initial['container_id']], environment, capture=True).strip()
            require(user == '65532:65532', 'container_executa_com_usuario_inesperado')
            runner.run(['docker', 'exec', initial['container_id'], '/app/stories-tech', '--healthcheck'],
                       environment, timeout=15)
            sql = deployment.sql_report(initial, report['started_at'])
            require(sql['unresolved'] == 0 and sql['n1_published'] == 0, 'publicacao_inesperada_no_fixture')
            check('migracao_readiness_modo_teto_retomada_e_hardening')

            stage = 'substituicao_e_rollback'
            replacement = deployment.apply('dry_run', second)
            require(replacement['image_id'] == second_id and replacement['container_id'] != initial['container_id'],
                    'imagem_nao_foi_substituida')
            active(replacement)
            rolled_back = deployment.apply('rollback')
            require(rolled_back['image_id'] == image_id, 'rollback_nao_restaurou_imagem')
            active(rolled_back)
            check('rollback_explicito_preserva_imagem_dados_e_pausa')

            stage = 'falha_real_de_migracao'
            needle = '${MIGRATION_DATABASE_URL:-${DATABASE_URL:?Defina DATABASE_URL com host postgres}}'
            require(original.count(needle) == 1, 'template_de_migracao_alterado')
            compose_path.write_text(original.replace(needle, '${ROLLOUT_BAD_MIGRATION_DATABASE_URL:?fixture}'))
            try:
                deployment.apply('dry_run', second)
            except OperationError as error:
                require(str(error) == 'implantacao_recusada_ou_revertida_com_pausa', 'recuperacao_de_migracao_falhou')
            else:
                raise OperationError('migracao_invalida_nao_foi_recusada')
            compose_path.write_text(original)
            active(rolled_back)
            check('falha_de_migracao_restaura_versao_anterior_sem_restaurar_banco')

            stage = 'falha_real_de_readiness'
            prefix, separator, app = original.partition('  app:\n')
            require(separator and app.count('${ADMIN_TOKEN:?Defina ADMIN_TOKEN}') == 1, 'template_administrativo_alterado')
            compose_path.write_text(prefix + separator + app.replace('${ADMIN_TOKEN:?Defina ADMIN_TOKEN}', '${ROLLOUT_WRONG_ADMIN_TOKEN:?fixture}'))
            try:
                deployment.apply('dry_run', second)
            except OperationError as error:
                require(str(error) == 'implantacao_recusada_ou_revertida_com_pausa', 'recuperacao_de_readiness_falhou')
            else:
                raise OperationError('token_administrativo_divergente_nao_foi_recusado')
            compose_path.write_text(original)
            active(rolled_back)
            check('falha_de_readiness_restaura_versao_anterior_pausada')

            stage = 'rede_e_auditoria'
            network = json.loads(runner.run(['docker', 'network', 'inspect', '--format', '{{json .}}', project + '_default'], environment, capture=True))
            require(network['Internal'] is True, 'rede_do_fixture_permite_egress')
            latest = read_json(deployment.record)['current']
            require(int(deployment.postgres(latest, ['-c', "SELECT count(*) FROM audit_log WHERE step='deployment_pause'"], read_only=True)) >= 6,
                    'pausas_nao_foram_auditadas')
            require(deployment.postgres(latest, ['-c', 'SELECT count(*) FROM publications WHERE NOT dry_run'], read_only=True).strip() == '0',
                    'fixture_criou_publicacao_real')
            report['shutdown_exit_codes'] = runner.shutdown_codes
            require(len(runner.shutdown_codes) >= 6 and all(code == 0 for code in runner.shutdown_codes),
                    'shutdown_normal_encerrou_com_erro')
            check('rede_interna_auditoria_e_zero_publicacoes_reais')
            report['passed'] = True
        except (OperationError, OSError, ValueError, KeyError, TypeError):
            report['failure_code'] = stage
            report['failed_command'] = runner.failed_command
            report['sanitized_command_error'] = runner.sanitized_error
            report['probes'] = admin.probes[-12:]
            try:
                raw_logs = runner.run([*command, 'logs', '--no-color', '--tail', '30', 'app'], environment, capture=True)
                report['secret_in_logs'] = any(value in raw_logs for value in private.values())
                for value in private.values():
                    raw_logs = raw_logs.replace(value, '[redigido]')
                report['sanitized_app_log'] = raw_logs
            except OperationError:
                report['sanitized_app_log'] = 'indisponivel'
            raise OperationError('ensaio_compose_falhou_em_' + stage) from None
        finally:
            compose_path.write_text(original)
            try:
                # Só o projeto aleatório criado neste teste pode perder seus volumes.
                require(re.fullmatch(r'bb-rollout-[a-f0-9]{16}', project), 'limpeza_fora_do_fixture')
                runner.run([*command, 'down', '--volumes', '--remove-orphans', '--timeout', '10'], environment, timeout=60)
                runner.run(['docker', 'image', 'rm', second_image], environment, timeout=30)
                report['cleanup_passed'] = True
            except OperationError:
                report['cleanup_passed'] = False
                report['passed'] = False
            report['duration_seconds'] = round(time.monotonic() - started, 3)
            atomic_json(report_path, report)
    require(report['passed'], 'ensaio_ou_limpeza_falhou')
    return report


def main():
    parser = argparse.ArgumentParser(description='Rollout real em Compose descartável e isolado')
    parser.add_argument('--image', required=True)
    parser.add_argument('--postgres-image')
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    try:
        result = run(args.image, args.postgres_image, args.report)
        print(json.dumps({'passed': result['passed'], 'checks': len(result['checks']), 'duration_seconds': result['duration_seconds']}))
        return 0
    except (OperationError, OSError):
        print('Ensaio recusado ou falhou; consulte a etapa no relatório, sem credenciais.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
