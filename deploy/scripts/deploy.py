#!/usr/bin/env python3
"""Rollout por digest: pausa auditada, migração, readiness e rollback sem restaurar banco."""
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile
import time
from urllib.parse import parse_qs, unquote, urlsplit
from acceptance import gate
from runtime import Admin, OperationError, Runner, atomic_json, read_json, timestamp, utc_now

IMAGE = re.compile(r'^ghcr\.io/lincolncyber-01/breaking-bytes@sha256:[a-f0-9]{64}$')
IDENTIFIER = re.compile(r'^[A-Za-z_][A-Za-z0-9_]{0,62}$')
PAUSE_SQL = "WITH changed AS (UPDATE pipeline_control SET paused=true,updated_at=now() WHERE id=true RETURNING id), recorded AS (INSERT INTO audit_log(step,detail) SELECT 'deployment_pause','{\"paused\":true,\"source\":\"rollout\"}'::jsonb FROM changed RETURNING id) SELECT count(*) FROM recorded"


def database_environment(environment, read_only=False):
    try:
        url = urlsplit(environment['DATABASE_URL'])
        query = parse_qs(url.query, strict_parsing=True)
        username, database = unquote(url.username or ''), unquote(url.path.lstrip('/'))
        if url.scheme not in {'postgres', 'postgresql'} or not url.hostname or not url.password or not IDENTIFIER.fullmatch(username) or not IDENTIFIER.fullmatch(database) or set(query) - {'sslmode'}:
            raise ValueError()
        mode = query.get('sslmode', ['prefer'])[0]
        if mode not in {'disable', 'allow', 'prefer', 'require', 'verify-ca', 'verify-full'}:
            raise ValueError()
        values = dict(environment, PGHOST=url.hostname, PGPORT=str(url.port or 5432), PGUSER=username,
                      PGPASSWORD=unquote(url.password), PGDATABASE=database, PGSSLMODE=mode,
                      PGSERVICEFILE='/dev/null', PGPASSFILE='/dev/null', PGCONNECT_TIMEOUT='5',
                      PGOPTIONS='-c statement_timeout=10000 -c lock_timeout=3000' + (' -c default_transaction_read_only=on' if read_only else ''))
        if any(any(c in values[key] for c in '\r\n\0') for key in ['PGHOST', 'PGUSER', 'PGPASSWORD', 'PGDATABASE']):
            raise ValueError()
        return values
    except (KeyError, ValueError, TypeError):
        raise OperationError('conexao_operacional_invalida') from None


def identity_hash(environment):
    names = ['IG_USER_ID', 'R2_ACCOUNT_ID', 'META_APP_ID', 'FB_PAGE_ID']
    return hashlib.sha256(json.dumps({name: environment.get(name, '') for name in names}, sort_keys=True).encode()).hexdigest()


def config_hash(directory):
    directory = Path(directory)
    hasher = hashlib.sha256()
    files = sorted(p for p in directory.iterdir() if p.suffix in {'.toml', '.json'})
    if not {'exemplo.toml', 'sources.toml', 'images.json'} <= {p.name for p in files} or len(files) > 32:
        raise OperationError('configuracao_incompleta')
    for path in files:
        if path.is_symlink() or not path.is_file() or path.stat().st_size > 2 * 1024 * 1024:
            raise OperationError('configuracao_invalida')
        hasher.update(path.name.encode() + b'\0' + path.read_bytes() + b'\0')
    return hasher.hexdigest()


@contextlib.contextmanager
def registry_environment(environment):
    """Docker recebe credencial por helper; config temporário contém apenas seu nome."""
    with tempfile.TemporaryDirectory(prefix='stories-registry-') as temporary:
        root = Path(temporary)
        configuration = {'auths': {}}
        result = dict(environment, DOCKER_CONFIG=str(root))
        token = environment.get('GITHUB_TOKEN') or environment.get('GHCR_READ_TOKEN')
        if token:
            helper = Path(__file__).resolve().parents[2] / 'tools/registry_credentials.py'
            target = root / 'docker-credential-breakingbytes'
            shutil.copyfile(helper, target)
            target.chmod(0o700)
            configuration = {'credHelpers': {'ghcr.io': 'breakingbytes'}}
            result.update(GITHUB_TOKEN=token, GITHUB_ACTOR=environment.get('GITHUB_ACTOR', 'lincolncyber-01'), PATH=str(root) + os.pathsep + environment.get('PATH', ''))
        (root / 'config.json').write_text(json.dumps(configuration))
        yield result


class Deployment:
    def __init__(self, source, configuration, state, environment=None, runner=None, admin=None):
        self.source = Path(source).resolve()
        self.configuration = Path(configuration).resolve()
        self.state = Path(state).resolve()
        self.environment = dict(os.environ if environment is None else environment)
        self.environment.pop('DOCKER_CONTEXT', None)
        self.environment.setdefault('DOCKER_HOST', 'unix:///var/run/docker.sock')
        if not self.environment['DOCKER_HOST'].startswith('unix://'):
            raise OperationError('daemon_remoto_nao_suportado')
        self.runner = runner or Runner()
        self.admin = admin or Admin()
        self.state.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.state.chmod(0o700)
        self.record = self.state / 'deployment.json'

    def compose(self, release):
        original = Path(release['snapshot'])
        snapshot = original.resolve()
        if not snapshot.is_relative_to(self.state / 'releases') or original.is_symlink() or config_hash(snapshot / 'config') != release['files_sha256'] or identity_hash(self.environment) != release['identity_sha256']:
            raise OperationError('snapshot_de_implantacao_divergente')
        return ['docker', 'compose', '--project-name', 'breaking-bytes', '-f', str(snapshot / 'deploy/docker-compose.yml')]

    def environment_for(self, release):
        return dict(self.environment, STORIES_IMAGE=release['image'],
                    STORIES__INSTAGRAM__DRY_RUN='true' if release['mode'] == 'dry_run' else 'false',
                    STORIES__PIPELINE__DAILY_CAP=str(release['cap']),
                    STORIES__PUBLISHING__RESERVED_SLOTS=str(release['reserved_slots']),
                    STORIES__COLLECTION__ENABLED='true', STORIES__LLM__ENABLED='true',
                    STORIES__OBSERVABILITY__ALERTS_ENABLED='true', STORIES__OBSERVABILITY__BACKUPS_EXPECTED='true')

    def snapshot(self, image, mode, cap):
        checksum = config_hash(self.configuration)
        root = self.state / 'releases' / (image.rsplit(':', 1)[-1][:16] + '-' + str(time.time_ns()))
        (root / 'deploy').mkdir(parents=True, mode=0o700)
        (root / 'config').mkdir(mode=0o755)
        (root / 'config').chmod(0o755)
        for path in self.configuration.iterdir():
            if path.suffix in {'.toml', '.json'}:
                target = root / 'config' / path.name
                shutil.copyfile(path, target)
                target.chmod(0o644)
        shutil.copyfile(self.source / 'deploy/docker-compose.yml', root / 'deploy/docker-compose.yml')
        return {'image': image, 'mode': mode, 'cap': cap, 'reserved_slots': min(5, max(1, cap // 4)),
                'files_sha256': checksum, 'identity_sha256': identity_hash(self.environment),
                'config_sha256': hashlib.sha256((checksum + identity_hash(self.environment)).encode()).hexdigest(),
                'snapshot': str(root), 'created_at': utc_now()}

    def inspect_image(self, image, environment, pull=True):
        if pull:
            self.runner.run(['docker', 'pull', image], environment, timeout=300)
        raw = self.runner.run(['docker', 'image', 'inspect', '--format', '{{.Id}} {{.Architecture}} {{index .Config.Labels "org.opencontainers.image.revision"}}', image], environment, timeout=15, capture=True)
        parts = raw.split()
        if len(parts) != 3 or not re.fullmatch(r'sha256:[a-f0-9]{64}', parts[0]) or parts[1] != 'amd64' or not re.fullmatch(r'[a-f0-9]{40}', parts[2]):
            raise OperationError('imagem_sem_procedencia_validada')
        return {'image_id': parts[0], 'revision': parts[2]}

    def postgres(self, release, arguments, read_only=False, timeout=20, input_data=None):
        environment = database_environment(self.environment_for(release), read_only)
        flags = []
        for key in ['PGHOST', 'PGPORT', 'PGUSER', 'PGPASSWORD', 'PGDATABASE', 'PGSSLMODE', 'PGCONNECT_TIMEOUT', 'PGOPTIONS', 'PGPASSFILE', 'PGSERVICEFILE']:
            # Sem '=valor': Docker herda do ambiente do processo, inclusive a senha.
            flags.extend(['-e', key])
        # -T é compatível também com versões que ainda grafam a opção longa --no-TTY.
        return self.runner.run([*self.compose(release), 'exec', '-T', *flags, 'postgres', 'psql', '-X', '-q', '-A', '-t', '--no-password', '-v', 'ON_ERROR_STOP=1', *arguments], environment, timeout=timeout, capture=True, input_data=input_data)

    def pause(self, release):
        if self.postgres(release, ['-c', PAUSE_SQL]).strip() != '1':
            raise OperationError('pausa_auditada_nao_confirmada')

    def sql_report(self, release, started_at):
        timestamp(started_at)
        sql = (self.source / 'deploy/sql/acceptance.sql').read_text()
        # A consulta vem de arquivo versionado; o parâmetro é passado com quoting do psql.
        raw = self.postgres(release, ['-v', 'window_start=' + started_at, '-f', '-'], read_only=True, input_data=sql.encode())
        try:
            value = json.loads(raw)
            if not isinstance(value, dict):
                raise ValueError()
            return value
        except ValueError:
            raise OperationError('relatorio_sql_invalido') from None

    def container(self, release, environment):
        identifier = self.runner.run([*self.compose(release), 'ps', '--status', 'running', '-q', 'app'], environment, timeout=15, capture=True).strip()
        if not re.fullmatch(r'[a-f0-9]{64}', identifier):
            raise OperationError('container_nao_confirmado')
        actual = self.runner.run(['docker', 'inspect', '--format', '{{.Image}}', identifier], environment, timeout=15, capture=True).strip()
        if actual != release['image_id']:
            raise OperationError('container_de_outra_imagem')
        return identifier

    def restore_previous(self, candidate, previous, original):
        candidate_environment = self.environment_for(candidate)
        self.runner.run([*self.compose(candidate), 'stop', '--timeout', '30', 'app'], candidate_environment, timeout=45)
        # Sem banco disponível não se reinicia uma imagem que possa publicar sem pausa.
        self.pause(candidate)
        if previous is None:
            return
        old_environment = self.environment_for(previous)
        self.runner.run([*self.compose(previous), 'up', '-d', '--no-deps', '--no-build', '--pull', 'never', '--force-recreate', 'app'], old_environment, timeout=90)
        self.admin.wait_ready(previous['mode'] == 'dry_run', previous['cap'])
        restored = dict(previous, container_id=self.container(previous, old_environment))
        atomic_json(self.record, {'schema': 1, 'current': restored, 'previous': original.get('previous'), 'rolled_back_at': utc_now()})

    def apply(self, mode, image=None, cap=10, receipt=None, resume=False):
        if mode not in {'dry_run', 'canary', 'promote', 'rollback'}:
            raise OperationError('modo_invalido')
        with (self.state / 'deployment.lock').open('a') as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise OperationError('implantacao_em_execucao') from None
            original = read_json(self.record) if self.record.exists() else {'schema': 1, 'current': None, 'previous': None}
            if original.get('schema') != 1:
                raise OperationError('estado_de_implantacao_invalido')
            previous = original.get('current')
            rollback = mode == 'rollback'
            if rollback:
                if original.get('previous') is None or resume:
                    raise OperationError('rollback_indisponivel_ou_retomada_automatica_recusada')
                candidate = dict(original['previous'])
                image = candidate['image']
            else:
                if not IMAGE.fullmatch(image or '') or mode == 'canary' and cap != 10 or mode == 'promote' and cap not in {20, 40, 80} or mode == 'dry_run' and cap != 10:
                    raise OperationError('imagem_ou_teto_invalido')
                if previous is None and mode != 'dry_run':
                    raise OperationError('primeira_implantacao_exige_simulacao')
                candidate = self.snapshot(image, mode, cap)
                if mode in {'canary', 'promote'}:
                    if receipt is None:
                        raise OperationError('recibo_de_aceite_obrigatorio')
                    required_stage = 'dry_run' if mode == 'canary' else 'canary' if cap == 20 else 'promoted'
                    required_cap = None if mode == 'canary' else {20: 10, 40: 20, 80: 40}[cap]
                    accepted = gate(receipt, image, candidate['config_sha256'], required_stage, required_cap)
                    candidate['acceptance_started_at'] = read_json(accepted['reports'][required_stage]['path'])['started_at']
            if not IMAGE.fullmatch(image or ''):
                raise OperationError('imagem_invalida')
            mutation_started = False
            with registry_environment(self.environment) as pull_environment:
                candidate.update(self.inspect_image(image, pull_environment, pull=not rollback))
            environment = self.environment_for(candidate)
            self.runner.run([*self.compose(candidate), 'config', '--quiet'], environment, timeout=20)
            try:
                if previous is not None:
                    self.pause(previous)
                    mutation_started = True
                    self.runner.run([*self.compose(previous), 'stop', '--timeout', '30', 'app'], self.environment_for(previous), timeout=45)
                else:
                    running = self.runner.run([*self.compose(candidate), 'ps', '--status', 'running', '-q', 'app'], environment, timeout=15, capture=True).strip()
                    if running:
                        raise OperationError('instalacao_existente_sem_estado')
                    self.runner.run([*self.compose(candidate), 'up', '-d', '--wait', '--wait-timeout', '90', 'postgres'], environment, timeout=120)
                if not rollback:
                    self.runner.run([*self.compose(candidate), 'run', '--rm', '--no-deps', '--pull', 'never', 'migrate'], environment, timeout=180)
                self.pause(candidate)
                mutation_started = True
                self.runner.run([*self.compose(candidate), 'up', '-d', '--no-deps', '--no-build', '--pull', 'never', '--force-recreate', 'app'], environment, timeout=90)
                self.admin.wait_ready(candidate['mode'] == 'dry_run', candidate['cap'])
                candidate['container_id'] = self.container(candidate, environment)
                if resume:
                    if candidate['mode'] != 'dry_run':
                        status = self.sql_report(candidate, candidate['acceptance_started_at'])
                        if status.get('unresolved') != 0 or status.get('dead_jobs') != 0 or status.get('backup_verified') is not True or status.get('restore_verified') is not True:
                            raise OperationError('operacao_pendente_impede_retomada')
                    self.admin.pause(False)
                atomic_json(self.record, {'schema': 1, 'current': candidate, 'previous': previous, 'deployed_at': utc_now()})
                return candidate
            except (OperationError, OSError, KeyError, ValueError):
                if mutation_started:
                    try:
                        self.restore_previous(candidate, previous, original)
                    except (OperationError, OSError, KeyError, ValueError):
                        atomic_json(self.state / 'failed-deployment.json', {'at': utc_now(), 'image': image, 'status': 'rollback_pendente_mantenha_workers_parados'})
                        raise OperationError('implantacao_falhou_e_rollback_exige_intervencao') from None
                raise OperationError('implantacao_recusada_ou_revertida_com_pausa') from None


def main():
    parser = argparse.ArgumentParser(description='Rollout e rollback por digest no host da VPS')
    parser.add_argument('--mode', choices=['dry_run', 'canary', 'promote', 'rollback'], required=True)
    parser.add_argument('--image')
    parser.add_argument('--cap', type=int, default=10)
    parser.add_argument('--receipt', type=Path)
    parser.add_argument('--resume', action='store_true')
    parser.add_argument('--config', type=Path, default=Path('/opt/breaking-bytes/config'))
    parser.add_argument('--state', type=Path, default=Path('/var/lib/breaking-bytes'))
    args = parser.parse_args()
    try:
        result = Deployment(Path(__file__).resolve().parents[2], args.config, args.state).apply(args.mode, args.image, args.cap, args.receipt, args.resume)
        print(json.dumps({'status': 'implantacao_validada', 'image': result['image'], 'mode': result['mode'], 'cap': result['cap'], 'resumed': args.resume}))
        return 0
    except (OperationError, OSError, KeyError, ValueError):
        print('Implantação recusada. Confira readiness, recibo e estado privado; mantenha a pausa durante recuperação.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
