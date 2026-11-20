"""Rollout por contrato: ordem, falhas, pausa e rollback com estado preservado."""
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from deploy import Deployment, OperationError, database_environment
from runtime import read_json

ROOT = Path(__file__).resolve().parents[2]
FIRST = 'ghcr.io/lincolncyber-01/breaking-bytes@sha256:' + 'a' * 64
SECOND = 'ghcr.io/lincolncyber-01/breaking-bytes@sha256:' + 'b' * 64
ENV = {'DATABASE_URL': 'postgres://stories_runtime:contract-only@postgres/stories', 'ADMIN_TOKEN': 'a' * 40, 'PATH': os.environ.get('PATH', '')}


class FakeRunner:
    def __init__(self):
        self.events = []
        self.active = False
        self.image_id = None
        self.container_number = 0
        self.fail = None
        self.seen_password_argument = False

    def run(self, args, environment=None, timeout=120, capture=False, input_data=None):
        self.seen_password_argument |= any('contract-only' in value for value in args)
        if args[:2] == ['docker', 'pull']:
            self.events.append('pull')
            if self.fail == 'pull':
                raise OperationError('comando_operacional_falhou')
        elif args[:3] == ['docker', 'image', 'inspect']:
            return 'sha256:' + args[-1][-64:] + ' amd64 ' + 'c' * 40
        elif args[:2] == ['docker', 'inspect']:
            return self.image_id
        elif 'psql' in args:
            if input_data is not None:
                self.events.append('sql_readonly')
                if 'default_transaction_read_only=on' not in environment['PGOPTIONS']:
                    raise AssertionError('consulta de aceite deve ser somente leitura')
                return json.dumps({'unresolved': 0, 'dead_jobs': 0, 'backup_verified': True, 'restore_verified': True})
            self.events.append('pause')
            return '1\n'
        elif 'ps' in args:
            return f'{self.container_number:064x}' if self.active else ''
        elif 'migrate' == args[-1]:
            self.events.append('migrate')
            if self.fail == 'migration':
                self.fail = None
                raise OperationError('comando_operacional_falhou')
        elif 'stop' in args:
            self.events.append('stop')
            self.active = False
        elif 'up' in args and args[-1] == 'app':
            self.events.append('up')
            self.active = True
            self.container_number += 1
            self.image_id = 'sha256:' + environment['STORIES_IMAGE'][-64:]
        elif 'up' in args and args[-1] == 'postgres':
            self.events.append('database')
        return ''


class FakeAdmin:
    def __init__(self, runner):
        self.runner, self.fail = runner, False

    def wait_ready(self, *_):
        self.runner.events.append('ready')
        if self.fail:
            self.fail = False
            raise OperationError('readiness_nao_confirmada')

    def pause(self, value):
        self.runner.events.append('resume' if not value else 'pause')


class DeploymentTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.runner = FakeRunner()
        self.admin = FakeAdmin(self.runner)
        self.deployment = Deployment(ROOT, ROOT / 'config', Path(self.temporary.name), ENV, self.runner, self.admin)

    def tearDown(self):
        self.temporary.cleanup()

    def test_migration_precedes_readiness_and_only_digest_is_accepted(self):
        current = self.deployment.apply('dry_run', FIRST, resume=True)
        events = self.runner.events
        self.assertLess(events.index('migrate'), events.index('up'))
        self.assertLess(events.index('pause'), events.index('up'))
        self.assertLess(events.index('ready'), events.index('resume'))
        self.assertEqual(current['cap'], 10)
        self.assertEqual(current['reserved_slots'], 2)
        self.assertEqual((Path(current['snapshot']) / 'config').stat().st_mode & 0o777, 0o755)
        self.assertFalse(self.runner.seen_password_argument)
        with self.assertRaises(OperationError):
            self.deployment.apply('dry_run', 'ghcr.io/lincolncyber-01/breaking-bytes:latest')

    def test_failed_migration_or_health_restores_previous_image_with_pause(self):
        first = self.deployment.apply('dry_run', FIRST)
        for failure in ['migration', 'health']:
            self.runner.events.clear()
            self.runner.fail = 'migration' if failure == 'migration' else None
            self.admin.fail = failure == 'health'
            with self.assertRaises(OperationError):
                self.deployment.apply('dry_run', SECOND, resume=True)
            record = read_json(self.deployment.record)
            self.assertEqual(record['current']['image'], first['image'])
            self.assertNotIn('resume', self.runner.events)
            self.assertEqual(self.runner.image_id, 'sha256:' + 'a' * 64)
            self.assertLess(self.runner.events.index('pause'), self.runner.events.index('stop'))

    def test_real_mode_requires_complete_receipt_and_ramp_cannot_jump(self):
        with self.assertRaises(OperationError):
            self.deployment.apply('canary', FIRST)
        self.assertEqual(self.runner.events, [])
        self.deployment.apply('dry_run', FIRST)
        with self.assertRaises(OperationError):
            self.deployment.apply('canary', FIRST, cap=20)
        with self.assertRaises(OperationError):
            self.deployment.apply('canary', FIRST)
        with patch('deploy.gate', side_effect=OperationError('aceite_de_outra_versao')):
            with self.assertRaises(OperationError):
                self.deployment.apply('promote', FIRST, cap=40, receipt=Path('absent.json'))

    def test_explicit_rollback_uses_cache_does_not_migrate_and_cannot_resume(self):
        self.deployment.apply('dry_run', FIRST)
        self.deployment.apply('dry_run', SECOND)
        self.runner.events.clear()
        current = self.deployment.apply('rollback')
        self.assertEqual(current['image'], FIRST)
        self.assertNotIn('migrate', self.runner.events)
        self.assertNotIn('pull', self.runner.events)
        self.assertNotIn('resume', self.runner.events)
        with self.assertRaises(OperationError):
            self.deployment.apply('rollback', resume=True)

    def test_password_is_environment_only_and_readonly_is_enforced(self):
        value = database_environment({'DATABASE_URL': 'postgres://stories_runtime:secret%40value@postgres/stories'}, True)
        self.assertEqual(value['PGPASSWORD'], 'secret@value')
        self.assertIn('default_transaction_read_only=on', value['PGOPTIONS'])
        for dsn in ['http://u:p@host/stories', 'postgres://u:p@host/stories?options=unsafe', 'postgres://u:p@host/stories%0a']:
            with self.assertRaises(OperationError):
                database_environment({'DATABASE_URL': dsn})
