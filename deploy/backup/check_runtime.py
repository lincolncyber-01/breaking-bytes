#!/usr/bin/env python3
"""Contrato offline dos clientes reais: SigV4, PUT/GET/listagem/DELETE e UUID seguro."""
import datetime as dt
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import threading
from urllib.parse import parse_qsl, quote, urlsplit
import uuid


class Contract(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    data = None
    calls = []
    failure = False

    def log_message(self, *_):
        pass

    def signed(self, body):
        match = re.fullmatch(r'AWS4-HMAC-SHA256 Credential=contract-access/(\d{8})/auto/s3/aws4_request, SignedHeaders=([a-z0-9;-]+), Signature=([a-f0-9]{64})', self.headers.get('Authorization', ''))
        if not match:
            return False
        day, names, signature = match.groups()
        parsed = urlsplit(self.path)
        query = '&'.join(sorted(quote(k, safe='-_.~') + '=' + quote(v, safe='-_.~') for k, v in parse_qsl(parsed.query, keep_blank_values=True)))
        headers = ''.join(name + ':' + ' '.join(self.headers.get(name, '').split()) + '\n' for name in names.split(';'))
        payload_hash = self.headers.get('X-Amz-Content-SHA256', hashlib.sha256(body).hexdigest())
        if payload_hash not in {hashlib.sha256(body).hexdigest(), 'UNSIGNED-PAYLOAD'}:
            return False
        canonical = '\n'.join([self.command, parsed.path, query, headers, names, payload_hash])
        scope = day + '/auto/s3/aws4_request'
        string = '\n'.join(['AWS4-HMAC-SHA256', self.headers.get('X-Amz-Date', ''), scope, hashlib.sha256(canonical.encode()).hexdigest()])
        key = b'AWS4contract-secret'
        for value in [day, 'auto', 's3', 'aws4_request']:
            key = hmac.digest(key, value.encode(), 'sha256')
        return hmac.compare_digest(hmac.new(key, string.encode(), hashlib.sha256).hexdigest(), signature)

    def reply(self, code, body=b''):
        self.send_response(code)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('ETag', '"contract"')
        self.end_headers()
        self.wfile.write(body)

    def operation(self):
        size = int(self.headers.get('Content-Length', '0'))
        if not 0 <= size <= 16384:
            type(self).failure = True
            self.reply(413)
            return
        body = self.rfile.read(size)
        if not self.signed(body):
            type(self).failure = True
            self.reply(403)
            return
        path = urlsplit(self.path)
        type(self).calls.append(self.command)
        if path.path == '/contract/health/contract' and self.command == 'PUT':
            type(self).data = body
            self.reply(200)
        elif path.path == '/contract/health/contract' and self.command == 'GET' and type(self).data is not None:
            self.reply(200, type(self).data)
        elif path.path == '/contract' and self.command == 'GET':
            now = dt.datetime.now(dt.timezone.utc).isoformat()
            self.reply(200, ('<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>contract</Name><Prefix>backups/</Prefix><KeyCount>1</KeyCount><IsTruncated>false</IsTruncated><Contents><Key>health/contract</Key><LastModified>' + now + '</LastModified><ETag>contract</ETag><Size>16</Size><StorageClass>STANDARD</StorageClass></Contents></ListBucketResult>').encode())
        elif path.path == '/contract/health/contract' and self.command == 'DELETE':
            type(self).data = None
            self.reply(204)
        else:
            type(self).failure = True
            self.reply(400)

    do_PUT = operation
    do_GET = operation
    do_DELETE = operation


def main():
    if '--check-minimal' in sys.argv:
        if any(importlib.util.find_spec(name) is not None for name in ['pip', '_uuid', 'readline', '_curses', '_sqlite3', '_tkinter']):
            raise RuntimeError('módulo opcional ainda presente')
    if uuid.uuid4().version != 4:
        raise RuntimeError('UUID de contrato inválido')
    server = ThreadingHTTPServer(('127.0.0.1', 0), Contract)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    environment = dict(os.environ, AWS_ACCESS_KEY_ID='contract-access', AWS_SECRET_ACCESS_KEY='contract-secret',
                       AWS_EC2_METADATA_DISABLED='true', AWS_CONFIG_FILE='/dev/null', AWS_SHARED_CREDENTIALS_FILE='/dev/null',
                       AWS_PAGER='', AWS_CLI_AUTO_PROMPT='off', AWS_MAX_ATTEMPTS='1',
                       AWS_REQUEST_CHECKSUM_CALCULATION='when_required', AWS_RESPONSE_CHECKSUM_VALIDATION='when_required',
                       HTTP_PROXY='http://127.0.0.1:9', HTTPS_PROXY='http://127.0.0.1:9', NO_PROXY='127.0.0.1')
    client = [os.environ.get('BACKUP_CHECK_AWS', 'aws'), '--endpoint-url', f'http://127.0.0.1:{server.server_port}',
              '--region', 'auto', '--cli-connect-timeout', '2', '--cli-read-timeout', '2', '--no-cli-pager', '--no-paginate', 's3api']
    try:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, target = root / 'source', root / 'target'
            source.write_bytes(b'backup-contract\n')
            for operation, args in [('put-object', ['--key', 'health/contract', '--body', str(source)]),
                                    ('get-object', ['--key', 'health/contract', str(target)]),
                                    ('list-objects-v2', ['--prefix', 'backups/']),
                                    ('delete-object', ['--key', 'health/contract'])]:
                result = subprocess.run([*client, operation, '--bucket', 'contract', *args], env=environment,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
                if result.returncode:
                    raise RuntimeError('cliente S3 recusou o contrato: ' + operation)
                if operation == 'list-objects-v2' and json.loads(result.stdout)['Contents'][0]['Key'] != 'health/contract':
                    raise RuntimeError('listagem divergente')
            if target.read_bytes() != source.read_bytes() or Contract.failure or Contract.calls != ['PUT', 'GET', 'GET', 'DELETE']:
                raise RuntimeError('contrato S3 divergente')
        print(json.dumps({'status': 'clientes_s3_e_sigv4_verificados', 'requests': 4, 'network': 'somente_loopback'}))
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)


if __name__ == '__main__':
    main()
