"""Primitivas operacionais: ambiente, arquivos privados e API administrativa local."""
import datetime as dt
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


class OperationError(Exception):
    """Código controlado, sem mensagens externas ou valores secretos."""


def utc_now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def timestamp(value):
    try:
        result = dt.datetime.fromisoformat(value.replace('Z', '+00:00'))
        if result.tzinfo is None:
            raise ValueError()
        return result
    except (ValueError, AttributeError, TypeError):
        raise OperationError('data_invalida') from None


def finite(value, minimum=0, maximum=10**15):
    return type(value) in (int, float) and math.isfinite(value) and minimum <= value <= maximum


def read_json(path, maximum=4 * 1024 * 1024):
    path = Path(path)
    try:
        if path.is_symlink() or not path.is_file() or path.stat().st_size > maximum:
            raise OperationError('arquivo_invalido')
        return json.loads(path.read_bytes(), parse_constant=lambda _: (_ for _ in ()).throw(ValueError()))
    except (OSError, ValueError):
        raise OperationError('arquivo_invalido') from None


def atomic_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    if path.is_symlink():
        raise OperationError('arquivo_invalido')
    raw = (json.dumps(value, indent=2, ensure_ascii=False, allow_nan=False) + '\n').encode()
    if len(raw) > 4 * 1024 * 1024:
        raise OperationError('relatorio_acima_do_limite')
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as output:
            temporary = Path(output.name)
            os.fchmod(output.fileno(), 0o600)
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        temporary = None
        descriptor = os.open(path.parent, os.O_DIRECTORY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


class Runner:
    def run(self, arguments, environment=None, timeout=120, capture=False, input_data=None):
        try:
            with tempfile.TemporaryFile() as output:
                subprocess.run(arguments, env=environment, stdout=output if capture else subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, timeout=timeout, check=True, input=input_data)
                if capture:
                    output.seek(0)
                    value = output.read(65537)
                    if len(value) > 65536:
                        raise OperationError('resposta_de_comando_excessiva')
                    return value.decode('utf-8')
            return ''
        except (OSError, subprocess.SubprocessError, UnicodeError):
            raise OperationError('comando_operacional_falhou') from None


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_):
        return None


class Admin:
    def __init__(self, token=None, port=8080):
        self.token = token if token is not None else os.environ.get('ADMIN_TOKEN', '')
        if not 32 <= len(self.token) <= 8192 or any(c in self.token for c in '\r\n\0') or not 1 <= port <= 65535:
            raise OperationError('credencial_administrativa_invalida')
        self.base = f'http://127.0.0.1:{port}'
        self.client = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def request(self, path, payload=None):
        if path not in {'/health/ready', '/admin/status', '/admin/stats', '/admin/pause'}:
            raise OperationError('rota_invalida')
        headers = {'Authorization': 'Bearer ' + self.token, 'Accept': 'application/json'}
        data = None
        if payload is not None:
            data = json.dumps(payload).encode()
            headers['Content-Type'] = 'application/json'
        request = urllib.request.Request(self.base + path, data=data, headers=headers,
                                         method='PUT' if payload is not None else 'GET')
        try:
            with self.client.open(request, timeout=5) as response:
                raw = response.read(1024 * 1024 + 1)
                if response.status != 200 or len(raw) > 1024 * 1024:
                    raise OperationError('resposta_administrativa_invalida')
            value = json.loads(raw, parse_constant=lambda _: (_ for _ in ()).throw(ValueError()))
            if not isinstance(value, dict):
                raise OperationError('resposta_administrativa_invalida')
            return value
        except (urllib.error.URLError, OSError, ValueError):
            raise OperationError('api_administrativa_indisponivel') from None

    def pause(self, value):
        result = self.request('/admin/pause', {'paused': value})
        if result.get('paused') is not value:
            raise OperationError('pausa_nao_confirmada')

    def wait_ready(self, dry_run, cap, timeout=120):
        deadline = time.monotonic() + timeout
        while True:
            try:
                health = self.request('/health/ready')
                status = self.request('/admin/status')
                if health.get('status') == 'ready' and status.get('dry_run') is dry_run and status.get('daily_cap') == cap:
                    return status
            except OperationError:
                pass
            if time.monotonic() >= deadline:
                raise OperationError('readiness_nao_confirmada')
            time.sleep(1)
