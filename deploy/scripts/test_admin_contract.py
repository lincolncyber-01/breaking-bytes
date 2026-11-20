"""Contrato HTTP real da administração: token em header, limites e redirecionamento."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
import unittest
from runtime import Admin, OperationError


class Handler(BaseHTTPRequestHandler):
    captured = []

    def log_message(self, *_):
        pass

    def do_GET(self):
        type(self).captured.append((self.path, self.headers.get('Authorization')))
        if self.path == '/admin/status':
            self.send_response(302)
            self.send_header('Location', 'https://example.invalid/secret')
            self.end_headers()
        else:
            self.send_response(200)
            self.end_headers()
            self.wfile.write(json.dumps({'status': 'ready'}).encode())

    def do_PUT(self):
        value = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        type(self).captured.append((self.path, self.headers.get('Authorization')))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(json.dumps(value).encode())


class AdminTests(unittest.TestCase):
    def test_authentication_uses_header_and_redirect_cannot_leak_it(self):
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            token = 'contract-only-administrative-token-40-bytes'
            client = Admin(token, server.server_port)
            client.request('/health/ready')
            client.pause(True)
            with self.assertRaises(OperationError):
                client.request('/admin/status')
            self.assertTrue(all(header == 'Bearer ' + token and token not in path for path, header in Handler.captured))
            with self.assertRaises(OperationError):
                client.request('/untrusted')
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
