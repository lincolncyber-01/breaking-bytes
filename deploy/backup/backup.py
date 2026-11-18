#!/usr/bin/env python3
"""Backup operacional: PostgreSQL 16 e AWS CLI oficial, sem segredos em arquivo ou argv."""
import argparse
import datetime as dt
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.parse
from zoneinfo import ZoneInfo

UTC = dt.timezone.utc
KEY = re.compile(r"^backups/(daily|weekly)/(\d{4}-\d{2}-\d{2})/([a-f0-9]{64})\.dump$")
TABLES = ("events", "publications", "jobs", "audit_log", "articles", "pipeline_control")


class BackupError(Exception):
    """Contém somente código controlado, nunca a mensagem de um subprocesso."""


def required(name):
    value = os.environ.get(name, "")
    if not value or len(value) > 8192 or any(c in value for c in "\x00\r\n"):
        raise BackupError("configuracao_ausente_ou_invalida")
    return value


def connection(value):
    try:
        url = urllib.parse.urlsplit(value)
        query = urllib.parse.parse_qs(url.query, strict_parsing=True)
        if url.scheme not in ("postgres", "postgresql") or not url.hostname or not url.username:
            raise ValueError()
        database = urllib.parse.unquote(url.path.lstrip("/"))
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,62}", database) or not url.password or set(query) - {"sslmode"}:
            raise ValueError()
        mode = query.get("sslmode", ["prefer"])[0]
        if mode not in ("disable", "allow", "prefer", "require", "verify-ca", "verify-full"):
            raise ValueError()
        values = {
            "PGHOST": url.hostname, "PGPORT": str(url.port or 5432),
            "PGUSER": urllib.parse.unquote(url.username), "PGDATABASE": database,
            "PGPASSWORD": urllib.parse.unquote(url.password or ""), "PGSSLMODE": mode,
            "PGCONNECT_TIMEOUT": "10", "PGSERVICEFILE": "/dev/null", "PGPASSFILE": "/dev/null",
            "PGOPTIONS": "-c statement_timeout=600000 -c lock_timeout=10000",
        }
        if any(any(c in v for c in "\x00\r\n") for v in values.values()):
            raise ValueError()
        return values
    except (ValueError, TypeError):
        raise BackupError("conexao_invalida") from None


def digest(path, maximum):
    size = path.stat().st_size
    if size < 1 or size > maximum:
        raise BackupError("tamanho_do_backup_invalido")
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest(), size


def valid_key(key):
    match = KEY.fullmatch(key)
    if not match:
        raise BackupError("chave_fora_do_namespace")
    try:
        day = dt.date.fromisoformat(match[2])
    except ValueError:
        raise BackupError("periodo_invalido") from None
    if match[1] == "weekly" and day.weekday() != 0:
        raise BackupError("periodo_semanal_invalido")
    return match, day


def retention(objects, today):
    """Mantém sete datas diárias e quatro semanas; ignora objetos não gerenciados."""
    groups = {"daily": {}, "weekly": {}}
    for item in objects:
        key = item.get("Key", "")
        if not isinstance(key, str) or not key.endswith(".dump.json"):
            continue
        try:
            match, day = valid_key(key[:-5])
            modified = dt.datetime.fromisoformat(item["LastModified"].replace("Z", "+00:00"))
            if modified.tzinfo is None or day > today:
                continue
        except (BackupError, KeyError, ValueError, TypeError, AttributeError):
            continue
        groups[match[1]].setdefault(day, []).append((modified, key))
    obsolete = []
    for kind, periods in groups.items():
        keep = 7 if kind == "daily" else 4
        for index, day in enumerate(sorted(periods, reverse=True)):
            versions = sorted(periods[day], reverse=True)
            obsolete.extend(key for _, key in versions[(1 if index < keep else 0):])
    return obsolete


def managed_orphans(objects, now):
    """Objetos sem manifesto há 48 h excedem o prazo máximo de qualquer operação."""
    keys = {item.get("Key") for item in objects if isinstance(item.get("Key"), str)}
    obsolete = []
    for item in objects:
        key = item.get("Key", "")
        if not isinstance(key, str) or key + ".json" in keys:
            continue
        try:
            valid_key(key)
            modified = dt.datetime.fromisoformat(item["LastModified"].replace("Z", "+00:00"))
            if modified.tzinfo is not None and modified < now - dt.timedelta(hours=48):
                obsolete.append(key)
        except (BackupError, KeyError, ValueError, TypeError, AttributeError):
            continue
    return obsolete


class Operations:
    def __init__(self, runner=None):
        account = required("BACKUP_R2_ACCOUNT_ID")
        if not re.fullmatch(r"[a-fA-F0-9]{32}", account):
            raise BackupError("conta_r2_invalida")
        self.bucket = os.environ.get("BACKUP_BUCKET", "stories-backups")
        if not re.fullmatch(r"[a-z0-9][a-z0-9-]{1,61}[a-z0-9]", self.bucket):
            raise BackupError("bucket_invalido")
        try:
            self.maximum = int(os.environ.get("BACKUP_MAX_BYTES", str(512 * 1024 * 1024)))
            timeout = int(os.environ.get("BACKUP_TIMEOUT_S", "900"))
        except ValueError:
            raise BackupError("limites_invalidos") from None
        if not 1024 * 1024 <= self.maximum <= 8 * 1024**3 or not 60 <= timeout <= 1800:
            raise BackupError("limites_invalidos")
        self.deadline = time.monotonic() + timeout
        self.endpoint = f"https://{account}.r2.cloudflarestorage.com"
        allowed = {"PATH", "HOME", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "TMPDIR", "SSL_CERT_FILE", "SSL_CERT_DIR", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "http_proxy", "https_proxy", "all_proxy", "no_proxy"}
        self.environment = {k: v for k, v in os.environ.items() if k in allowed}
        self.environment.update({
            "AWS_ACCESS_KEY_ID": required("BACKUP_R2_ACCESS_KEY_ID"),
            "AWS_SECRET_ACCESS_KEY": required("BACKUP_R2_SECRET_ACCESS_KEY"),
            "AWS_DEFAULT_REGION": "auto", "AWS_EC2_METADATA_DISABLED": "true",
            "AWS_SHARED_CREDENTIALS_FILE": "/dev/null", "AWS_CONFIG_FILE": "/dev/null",
            "AWS_PAGER": "", "AWS_CLI_AUTO_PROMPT": "off", "AWS_MAX_ATTEMPTS": "3", "AWS_RETRY_MODE": "standard",
            "AWS_REQUEST_CHECKSUM_CALCULATION": "when_required",
            "AWS_RESPONSE_CHECKSUM_VALIDATION": "when_required",
        })
        self.runner = runner or self.execute

    def execute(self, args, environment=None):
        per_call = 600 if args[0] in ("pg_dump", "pg_restore") else 120
        remaining = min(per_call, self.deadline - time.monotonic())
        if remaining <= 0:
            raise BackupError("prazo_total_excedido")

        def limits():
            resource.setrlimit(resource.RLIMIT_FSIZE, (self.maximum, self.maximum))

        try:
            with tempfile.TemporaryFile() as output:
                subprocess.run(args, env=environment or self.environment, stdin=subprocess.DEVNULL,
                               stdout=output, stderr=subprocess.DEVNULL, timeout=remaining,
                               check=True, preexec_fn=limits)
                output.seek(0)
                value = output.read(2 * 1024 * 1024 + 1)
                if len(value) > 2 * 1024 * 1024:
                    raise BackupError("resposta_operacional_acima_do_limite")
                return value
        except (OSError, subprocess.SubprocessError):
            raise BackupError("comando_operacional_falhou") from None

    def sql(self, dsn, statement):
        environment = self.environment.copy()
        # PostgreSQL não recebe credenciais do R2 e a AWS CLI não recebe credenciais PostgreSQL.
        environment = {k: v for k, v in environment.items() if not k.startswith("AWS_")}
        environment.update(connection(dsn))
        return self.runner(["psql", "-X", "-A", "-t", "--no-password", "-v", "ON_ERROR_STOP=1", "-c", statement], environment).decode("utf-8").strip()

    def aws(self, operation, *args):
        response = self.runner(["aws", "--endpoint-url", self.endpoint, "--region", "auto",
                                "--cli-connect-timeout", "10", "--cli-read-timeout", "30",
                                "--no-cli-pager", "--no-paginate", "s3api", operation,
                                "--bucket", self.bucket, *args], self.environment)
        try:
            value = json.loads(response or b"{}")
            if not isinstance(value, dict):
                raise ValueError()
            return value
        except (ValueError, UnicodeError):
            raise BackupError("resposta_aws_invalida") from None

    def objects(self):
        objects, continuation = [], None
        for _ in range(128):
            args = ["--prefix", "backups/", "--max-keys", "1000"]
            if continuation:
                args.append("--continuation-token=" + continuation)
            value = self.aws("list-objects-v2", *args)
            page = value.get("Contents", [])
            if not isinstance(page, list) or len(page) > 1000 or any(not isinstance(x, dict) for x in page):
                raise BackupError("listagem_invalida")
            objects.extend(page)
            if value.get("IsTruncated") is not True:
                return objects
            continuation = value.get("NextContinuationToken")
            if not isinstance(continuation, str) or not 1 <= len(continuation) <= 2048:
                raise BackupError("paginacao_invalida")
        raise BackupError("listagem_excedeu_limite")

    def upload(self, file, key, manifest, temporary):
        valid_key(key)
        self.aws("put-object", "--key", key, "--body", str(file), "--content-type", "application/octet-stream")
        downloaded = temporary / "readback.dump"
        self.aws("get-object", "--key", key, str(downloaded))
        checksum, size = digest(downloaded, self.maximum)
        downloaded.unlink()
        if checksum != manifest["sha256"] or size != manifest["byte_length"]:
            raise BackupError("conferencia_remota_falhou")
        descriptor = temporary / "manifest.json"
        descriptor.write_text(json.dumps(manifest, sort_keys=True), encoding="utf-8")
        self.aws("put-object", "--key", key + ".json", "--body", str(descriptor), "--content-type", "application/json")
        # O manifesto também precisa sobreviver à leitura antes da confirmação local.
        descriptor.unlink()
        self.aws("get-object", "--key", key + ".json", str(descriptor))
        if descriptor.stat().st_size > 16384 or json.loads(descriptor.read_text()) != manifest:
            raise BackupError("manifesto_remoto_divergente")
        descriptor.unlink()

    def backup(self, temporary, now):
        source = required("BACKUP_DATABASE_URL")
        source_id = self.sql(source, "SELECT system_identifier::text FROM pg_control_system()")
        if not re.fullmatch(r"[0-9]{1,20}", source_id):
            raise BackupError("identidade_postgres_invalida")
        migration = self.sql(source, "SELECT COALESCE(max(version),0) FROM _sqlx_migrations WHERE success")
        if not migration.isdecimal() or int(migration) < 9:
            raise BackupError("schema_de_backup_antigo")
        dump = temporary / "backup.dump"
        environment = {k: v for k, v in self.environment.items() if not k.startswith("AWS_")}
        environment.update(connection(source))
        self.runner(["pg_dump", "--format=custom", "--no-owner", "--no-acl", "--file", str(dump)], environment)
        checksum, size = digest(dump, self.maximum)
        local = now.astimezone(ZoneInfo("America/Bahia")).date()
        key = f"backups/daily/{local.isoformat()}/{checksum}.dump"
        manifest = {"format": 1, "object_key": key, "sha256": checksum, "byte_length": size,
                    "source_system_id": source_id, "schema_migration": int(migration),
                    "created_at": now.isoformat()}
        self.upload(dump, key, manifest, temporary)
        week = (local - dt.timedelta(days=local.weekday())).isoformat()
        existing = self.objects()
        if not any(isinstance(x.get("Key"), str) and x["Key"].startswith(f"backups/weekly/{week}/") and x["Key"].endswith(".dump.json") for x in existing):
            weekly = f"backups/weekly/{week}/{checksum}.dump"
            self.upload(dump, weekly, dict(manifest, object_key=weekly), temporary)
        self.sql(source, f"INSERT INTO operational_backups(object_key,byte_length,sha256) VALUES('{key}',{size},'{checksum}') ON CONFLICT DO NOTHING")
        # Só entra em retenção depois de verificar o backup novo por GET.
        current_objects = self.objects()
        for orphan in managed_orphans(current_objects, now):
            self.aws("delete-object", "--key", orphan)
        for obsolete in retention(current_objects, local):
            self.aws("delete-object", "--key", obsolete[:-5])
            self.aws("delete-object", "--key", obsolete)
        return {"status": "backup_verificado", "object_key": key, "byte_length": size, "sha256": checksum}

    def restore(self, temporary, key):
        match, _ = valid_key(key)
        descriptor = temporary / "manifest.json"
        self.aws("get-object", "--key", key + ".json", str(descriptor))
        if descriptor.stat().st_size > 16384:
            raise BackupError("manifesto_acima_do_limite")
        try:
            manifest = json.loads(descriptor.read_text())
            if set(manifest) != {"format", "object_key", "sha256", "byte_length", "source_system_id", "schema_migration", "created_at"}:
                raise ValueError()
            created = dt.datetime.fromisoformat(manifest["created_at"])
            if created.tzinfo is None or created > dt.datetime.now(UTC) + dt.timedelta(minutes=5):
                raise ValueError()
            if type(manifest["format"]) is not int or manifest["format"] != 1 or manifest["object_key"] != key or manifest["sha256"] != match[3]:
                raise ValueError()
            if type(manifest["byte_length"]) is not int or not 1 <= manifest["byte_length"] <= self.maximum:
                raise ValueError()
            if type(manifest["schema_migration"]) is not int or manifest["schema_migration"] < 9 or not re.fullmatch(r"[0-9]{1,20}", manifest["source_system_id"]):
                raise ValueError()
        except (ValueError, TypeError, KeyError):
            raise BackupError("manifesto_invalido") from None
        target = required("RESTORE_DATABASE_URL")
        environment = connection(target)
        if not environment["PGDATABASE"].endswith("_restore_check"):
            raise BackupError("restauracao_exige_banco_isolado")
        target_id = self.sql(target, "SELECT system_identifier::text FROM pg_control_system()")
        if not re.fullmatch(r"[0-9]{1,20}", target_id) or target_id == manifest["source_system_id"]:
            raise BackupError("restauracao_exige_outro_cluster")
        if self.sql(target, "SELECT count(*) FROM pg_tables WHERE schemaname='public'") != "0":
            raise BackupError("restauracao_exige_banco_vazio")
        dump = temporary / "restore.dump"
        self.aws("get-object", "--key", key, str(dump))
        checksum, size = digest(dump, self.maximum)
        if checksum != manifest["sha256"] or size != manifest["byte_length"]:
            raise BackupError("checksum_de_restauracao_divergente")
        pg_environment = {k: v for k, v in self.environment.items() if not k.startswith("AWS_")}
        pg_environment.update(environment)
        self.runner(["pg_restore", "--exit-on-error", "--no-owner", "--no-acl", "--dbname", environment["PGDATABASE"], str(dump)], pg_environment)
        for table in TABLES:
            self.sql(target, f"SELECT count(*) FROM {table}")
        # O cliente nunca inicia trabalhadores usando o ledger restaurado.
        self.sql(target, "UPDATE pipeline_control SET paused=true,restore_hold_until=now()+interval '24 hours'; UPDATE publication_watches SET expires_at=now() WHERE expires_at>now(); INSERT INTO audit_log(step,detail) VALUES('restore_quarantine','{\"hold_hours\":24}')")
        self.sql(target, f"INSERT INTO operational_restore_checks(object_key,target_system_id) VALUES('{key}','{target_id}')")
        source = os.environ.get("BACKUP_DATABASE_URL")
        if source:
            self.sql(source, f"INSERT INTO operational_restore_checks(object_key,target_system_id) VALUES('{key}','{target_id}')")
        return {"status": "restauracao_verificada_e_pausada", "object_key": key, "target_system_id": target_id}


def main():
    parser = argparse.ArgumentParser(description="Backup R2 e restauração PostgreSQL isolada")
    parser.add_argument("mode", choices=("backup", "restore-check"))
    parser.add_argument("--key")
    args = parser.parse_args()
    os.umask(0o077)
    try:
        operations = Operations()
        directory = Path(os.environ.get("BACKUP_WORK_DIR", "/tmp/stories-backup"))
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        if directory.is_symlink() or directory.stat().st_uid != os.getuid():
            raise BackupError("diretorio_operacional_invalido")
        directory.chmod(0o700)
        lock_path = Path(os.environ.get("BACKUP_LOCK_FILE", str(directory / "exclusive.lock")))
        if not lock_path.parent.is_dir() or lock_path.is_symlink():
            raise BackupError("diretorio_do_lock_invalido")
        with lock_path.open("a") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise BackupError("operacao_ja_em_execucao") from None
            # O lock é compartilhado entre os contêineres operacionais na mesma VPS.
            for previous in directory.glob("run-*"):
                if previous.is_dir() and not previous.is_symlink() and previous.stat().st_uid == os.getuid():
                    shutil.rmtree(previous)
            if shutil.disk_usage(directory).free < operations.maximum * 2 + 64 * 1024 * 1024:
                raise BackupError("espaco_insuficiente_para_conferencia")
            with tempfile.TemporaryDirectory(prefix="run-", dir=directory) as temporary:
                if args.mode == "backup":
                    result = operations.backup(Path(temporary), dt.datetime.now(UTC))
                else:
                    key = args.key
                    if key is None:
                        candidates = [x for x in operations.objects() if isinstance(x.get("Key"), str) and x["Key"].startswith("backups/daily/") and x["Key"].endswith(".dump.json")]
                        if not candidates:
                            raise BackupError("nenhum_backup_concluido")
                        key = max(candidates, key=lambda x: (x.get("LastModified", ""), x["Key"]))["Key"][:-5]
                    result = operations.restore(Path(temporary), key)
        print(json.dumps(result, sort_keys=True))
        return 0
    except BackupError as error:
        print(json.dumps({"status": "falha", "code": str(error)}), file=sys.stderr)
        return 1
    except (OSError, ValueError, TypeError, KeyError, OverflowError, UnicodeError):
        print(json.dumps({"status": "falha", "code": "operacao_invalida"}), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
