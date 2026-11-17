"""Contratos sem credenciais reais; integração opcional usa dois clusters Docker sucessivos."""
import datetime as dt
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("backup", Path(__file__).with_name("backup.py"))
backup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(backup)
ENV = {"BACKUP_R2_ACCOUNT_ID": "a" * 32, "BACKUP_R2_ACCESS_KEY_ID": "contract-access",
       "BACKUP_R2_SECRET_ACCESS_KEY": "contract-secret", "BACKUP_DATABASE_URL": "postgres://user:contract@localhost/stories",
       "RESTORE_DATABASE_URL": "postgres://user:contract@localhost/stories_restore_check", "BACKUP_MAX_BYTES": str(16 * 1024 * 1024)}


class Fake:
    def __init__(self):
        self.objects = {}
        self.calls = []
        self.modified = {}
        self.corrupt = False
        self.source_id = "123456789"
        self.target_id = "987654321"
        self.target_tables = "0"
        self.sql_statements = []

    def __call__(self, args, environment=None):
        self.calls.append(args)
        if args[0] == "aws":
            assert "PGPASSWORD" not in environment
            operation = args[args.index("s3api") + 1]
            key = args[args.index("--key") + 1] if "--key" in args else None
            if operation == "put-object":
                self.objects[key] = Path(args[args.index("--body") + 1]).read_bytes()
                self.modified[key] = dt.datetime.now(backup.UTC).isoformat()
            elif operation == "get-object":
                Path(args[-1]).write_bytes(b"tampered" if self.corrupt and key.endswith(".dump") else self.objects[key])
            elif operation == "delete-object":
                self.objects.pop(key, None)
            elif operation == "list-objects-v2":
                return json.dumps({"Contents": [{"Key": k, "LastModified": self.modified[k]} for k in self.objects]}).encode()
            else:
                raise AssertionError("operação AWS inesperada")
            return b"{}"
        assert "AWS_SECRET_ACCESS_KEY" not in environment
        if args[0] == "pg_dump":
            Path(args[args.index("--file") + 1]).write_bytes(b"custom-format-fixture")
        elif args[0] == "pg_restore":
            self.target_tables = "6"
        elif args[0] == "psql":
            statement = args[-1]
            self.sql_statements.append(statement)
            if "pg_control_system" in statement:
                return (self.target_id if environment["PGDATABASE"].endswith("_restore_check") else self.source_id).encode()
            if "max(version)" in statement:
                return b"9"
            if "pg_tables" in statement:
                return self.target_tables.encode()
            return b"1"
        return b""


class BackupTests(unittest.TestCase):
    def test_retention_preserves_distinct_days_weeks_and_one_version_per_period(self):
        now = dt.datetime(2026, 11, 17, tzinfo=backup.UTC)
        today = now.date()
        objects = []
        for kind, count, step in [("daily", 10, 1), ("weekly", 6, 7)]:
            start = today if kind == "daily" else today - dt.timedelta(days=today.weekday())
            for index in range(count):
                day = start - dt.timedelta(days=index * step)
                objects.append({"Key": f"backups/{kind}/{day}/{'a' * 64}.dump.json", "LastModified": now.isoformat()})
        objects.append({"Key": f"backups/daily/{today}/{'b' * 64}.dump.json", "LastModified": (now - dt.timedelta(hours=1)).isoformat()})
        objects.append({"Key": "other/private.dump.json", "LastModified": now.isoformat()})
        obsolete = backup.retention(objects, today)
        self.assertEqual(len(obsolete), 6)
        self.assertTrue(all(k.startswith("backups/") for k in obsolete))
        self.assertFalse(any(f"backups/daily/{today}/{'a' * 64}" in k for k in obsolete))

    def test_orphan_collection_requires_managed_key_no_manifest_and_48_hours(self):
        now=dt.datetime.now(backup.UTC)
        key=f"backups/daily/{now.date()}/{'a'*64}.dump"
        old=(now-dt.timedelta(hours=49)).isoformat()
        self.assertEqual(backup.managed_orphans([{"Key":key,"LastModified":old}],now),[key])
        self.assertEqual(backup.managed_orphans([{"Key":key,"LastModified":old},{"Key":key+".json"}],now),[])
        self.assertEqual(backup.managed_orphans([{"Key":key,"LastModified":now.isoformat()}],now),[])
        self.assertEqual(backup.managed_orphans([{"Key":"external/object","LastModified":old}],now),[])

    def test_connection_never_puts_password_in_process_arguments(self):
        value = backup.connection("postgres://user:secret%40value@db:5432/stories?sslmode=require")
        self.assertEqual(value["PGPASSWORD"], "secret@value")
        self.assertEqual(value["PGSSLMODE"], "require")
        self.assertEqual(value["PGPASSFILE"], "/dev/null")
        for url in ["http://user:secret@db/stories", "postgres://user:secret@db/host%3Dprod_restore_check", "postgres://user:secret@db/stories?options=unsafe"]:
            with self.assertRaises(backup.BackupError):
                backup.connection(url)

    def test_put_get_manifest_restore_and_quarantine_contract(self):
        fake = Fake()
        with patch.dict(os.environ, ENV, clear=True), tempfile.TemporaryDirectory() as directory:
            operations = backup.Operations(fake)
            root = Path(directory)
            result = operations.backup(root, dt.datetime(2026, 11, 17, tzinfo=backup.UTC))
            self.assertEqual(len([k for k in fake.objects if k.endswith(".dump.json")]), 2)
            restored = operations.restore(root, result["object_key"])
            self.assertEqual(restored["status"], "restauracao_verificada_e_pausada")
            self.assertTrue(any("restore_hold_until=now()+interval '24 hours'" in s for s in fake.sql_statements))
            self.assertFalse(any("contract-secret" in " ".join(args) or "user:contract" in " ".join(args) for args in fake.calls))

    def test_upload_readback_failure_preserves_older_backups(self):
        fake = Fake()
        fake.corrupt = True
        with patch.dict(os.environ, ENV, clear=True), tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(backup.BackupError, "conferencia_remota"):
                backup.Operations(fake).backup(Path(directory), dt.datetime.now(backup.UTC))
        self.assertFalse(any("delete-object" in args for args in fake.calls))
        self.assertFalse(any(k.endswith(".json") for k in fake.objects))

    def test_restore_rejects_same_cluster_nonempty_database_and_tampered_dump(self):
        fake = Fake()
        with patch.dict(os.environ, ENV, clear=True), tempfile.TemporaryDirectory() as directory:
            operations = backup.Operations(fake)
            root = Path(directory)
            key = operations.backup(root, dt.datetime.now(backup.UTC))["object_key"]
            fake.target_id = fake.source_id
            with self.assertRaisesRegex(backup.BackupError, "outro_cluster"):
                operations.restore(root, key)
            fake.target_id = "987654321"
            fake.target_tables = "1"
            with self.assertRaisesRegex(backup.BackupError, "banco_vazio"):
                operations.restore(root, key)
            fake.target_tables = "0"
            fake.corrupt = True
            with self.assertRaisesRegex(backup.BackupError, "checksum"):
                operations.restore(root, key)
            self.assertFalse(any(args[0] == "pg_restore" for args in fake.calls))

    def test_malformed_key_cannot_select_or_delete_another_namespace(self):
        for key in ["../database.dump", "backups/daily/2026-99-99/" + "a" * 64 + ".dump", "backups/weekly/2026-11-17/" + "a" * 64 + ".dump"]:
            with self.assertRaises(backup.BackupError):
                backup.valid_key(key)
        self.assertEqual(backup.retention([{"Key": "../sensitive.dump.json"}], dt.date.today()), [])

    def test_provider_error_does_not_reveal_url_credentials_or_response(self):
        with patch.dict(os.environ, ENV, clear=True):
            operations = backup.Operations()
            with patch("subprocess.run", side_effect=subprocess.CalledProcessError(1, ["aws", "secret"], stderr=b"provider-secret")):
                with self.assertRaises(backup.BackupError) as caught:
                    operations.execute(["aws"])
                self.assertEqual(str(caught.exception), "comando_operacional_falhou")


class DockerRunner(Fake):
    def __init__(self):
        super().__init__()
        self.container = None

    def __call__(self, args, environment=None):
        if args[0] == "aws":
            return super().__call__(args, environment)
        command = ["docker", "exec", "-i", "-e", "PGHOST=127.0.0.1", "-e", "PGUSER=postgres",
                   "-e", "PGPASSWORD=integration-only", "-e", "PGDATABASE=" + environment["PGDATABASE"], self.container]
        if args[0] == "pg_dump":
            position = args.index("--file")
            with Path(args[position + 1]).open("wb") as output:
                subprocess.run(command + args[:position] + args[position + 2:], stdout=output, stderr=subprocess.PIPE, check=True, timeout=60)
            return b""
        if args[0] == "pg_restore":
            with Path(args[-1]).open("rb") as source:
                return subprocess.run(command + args[:-1], stdin=source, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=60).stdout
        return subprocess.run(command + args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=60).stdout

    def start(self, database):
        image = os.environ.get("STORIES_TEST_POSTGRES_IMAGE", "pgvector/pgvector") + ":0.8.2-pg16"
        self.container = subprocess.run(["docker", "run", "--detach", "--memory", "256m", "--cpus", "0.5", "-e", "POSTGRES_PASSWORD=integration-only", "-e", "POSTGRES_DB=" + database, image], stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=60).stdout.decode().strip()
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            ready = subprocess.run(["docker", "exec", self.container, "pg_isready", "-h", "127.0.0.1", "-U", "postgres", "-d", database], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
            if ready.returncode == 0:
                return
            time.sleep(0.1)
        raise AssertionError("Postgres de integração indisponível")

    def stop(self):
        if self.container:
            subprocess.run(["docker", "rm", "--force", self.container], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True, timeout=30)
            self.container = None


@unittest.skipUnless(os.environ.get("BACKUP_INTEGRATION") == "1", "ative BACKUP_INTEGRATION=1 para dois clusters Docker sucessivos")
class PostgresRestoreTests(unittest.TestCase):
    def test_actual_pg_dump_restore_preserves_rows_and_installs_quarantine(self):
        fake = DockerRunner()
        root = Path(__file__).resolve().parents[2]
        try:
            fake.start("stories")
            # Estrutura da tabela de migrações conferida no SQLx 0.9; as SQLs são as reais do projeto.
            statements = ["CREATE TABLE _sqlx_migrations(version bigint PRIMARY KEY,description text NOT NULL,installed_on timestamptz NOT NULL DEFAULT now(),success bool NOT NULL,checksum bytea NOT NULL,execution_time bigint NOT NULL);"]
            for migration in sorted((root / "migrations").glob("*.sql")):
                statements.append(migration.read_text())
                checksum = hashlib.sha384(migration.read_bytes()).hexdigest()
                version = int(migration.name.split("_")[0])
                statements.append(f"INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES({version},'integration',true,decode('{checksum}','hex'),0);")
            statements.append("INSERT INTO events DEFAULT VALUES;")
            subprocess.run(["docker", "exec", "-i", fake.container, "psql", "-U", "postgres", "-d", "stories", "-v", "ON_ERROR_STOP=1"], input="\n".join(statements).encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=60)
            with patch.dict(os.environ, ENV), tempfile.TemporaryDirectory() as directory:
                operations = backup.Operations(fake)
                key = operations.backup(Path(directory), dt.datetime.now(backup.UTC))["object_key"]
                fake.stop()
                os.environ.pop("BACKUP_DATABASE_URL",None)
                fake.start("stories_restore_check")
                operations.restore(Path(directory), key)
                env = backup.connection(ENV["RESTORE_DATABASE_URL"])
                count = fake(["psql", "-X", "-A", "-t", "-c", "SELECT count(*) FROM events"], env).strip()
                self.assertEqual(count, b"1")
                state = fake(["psql", "-X", "-A", "-t", "-c", "SELECT paused AND restore_hold_until>now()+interval '23 hours' FROM pipeline_control"], env).strip()
                self.assertEqual(state, b"t")
                result = subprocess.run(["docker", "exec", fake.container, "psql", "-U", "postgres", "-d", "stories_restore_check", "-v", "ON_ERROR_STOP=1", "-c", "UPDATE pipeline_control SET paused=false"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
                self.assertNotEqual(result.returncode, 0)
        finally:
            fake.stop()


if __name__ == "__main__":
    unittest.main()
