"""Executa pausa auditada e consulta de aceite em Postgres real, sem serviços remotos."""
import datetime as dt
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from deploy import Deployment
from runtime import OperationError

ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(os.environ.get('OPERATIONS_INTEGRATION') == '1', 'ative OPERATIONS_INTEGRATION=1 para o contrato Postgres real')
class SqlIntegrationTests(unittest.TestCase):
    def test_readonly_psql_variables_source_latency_and_audited_pause(self):
        spec = importlib.util.spec_from_file_location('backup_contract', ROOT / 'deploy/backup/test_backup.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        database = module.DockerRunner()
        try:
            database.start('stories')
            statements = '\n'.join(path.read_text() for path in sorted((ROOT / 'migrations').glob('*.sql')))
            statements += "\nINSERT INTO sources(name,layer,kind,url,weight,interval_s) VALUES('ContractPrimary','A','rss','https://example.com/feed',5,60);\n"
            statements += "INSERT INTO articles(source_id,url_canonical,title,title_hash,published_at,detected_at) SELECT id,'https://example.com/news','Rust anuncia versão 1.100',decode(repeat('00',32),'hex'),now()-interval '60 seconds',now()-interval '30 seconds' FROM sources WHERE name='ContractPrimary';"
            subprocess.run(['docker', 'exec', '-i', database.container, 'psql', '-U', 'postgres', '-d', 'stories', '-v', 'ON_ERROR_STOP=1'], input=statements.encode(), stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True, timeout=30)

            class ActualRunner:
                def run(self, arguments, environment=None, timeout=20, capture=False, input_data=None):
                    self_environment = dict(os.environ, **environment)
                    self_environment.update(PGHOST='127.0.0.1', PGPORT='5432')
                    flags = []
                    for name in ['PGHOST', 'PGPORT', 'PGUSER', 'PGPASSWORD', 'PGDATABASE', 'PGOPTIONS', 'PGCONNECT_TIMEOUT']:
                        flags += ['-e', name]
                    index = arguments.index('psql')
                    result = subprocess.run(['docker', 'exec', '-i', *flags, database.container, *arguments[index:]], env=self_environment,
                                             input=input_data, capture_output=True, timeout=timeout)
                    if result.returncode:
                        raise OperationError('comando_operacional_falhou')
                    return result.stdout.decode()

            with tempfile.TemporaryDirectory() as temporary:
                environment = dict(os.environ, DATABASE_URL='postgres://postgres:integration-only@127.0.0.1/stories')
                deployment = Deployment(ROOT, ROOT / 'config', Path(temporary), environment, ActualRunner(), object())
                release = deployment.snapshot('ghcr.io/lincolncyber-01/breaking-bytes@sha256:' + 'a' * 64, 'dry_run', 10)
                since = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(hours=1)).isoformat()
                value = deployment.sql_report(release, since)
                self.assertEqual(value['n1_articles'], 1)
                self.assertAlmostEqual(value['n1_detect_p95_seconds'], 30, places=1)
                self.assertEqual(value['n1_published'], 0)
                self.assertFalse(value['backup_verified'])
                deployment.pause(release)
                state = json.loads(deployment.postgres(release, ['-c', "SELECT json_build_object('paused',(SELECT paused FROM pipeline_control),'audits',(SELECT count(*) FROM audit_log WHERE step='deployment_pause'))"], read_only=True))
                self.assertTrue(state['paused'])
                self.assertEqual(state['audits'], 1)
                with self.assertRaises(OperationError):
                    deployment.postgres(release, ['-c', 'INSERT INTO events DEFAULT VALUES'], read_only=True)
        finally:
            database.stop()
