"""Gates de release e credenciais sem arquivo ou stdout de diagnóstico sensível."""
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from check_release_checks import REQUIRED, verify
from docker_registry import configured
from registry_tag import available


class Response(io.BytesIO):
    def __init__(self, body=b'', status=200):
        super().__init__(body)
        self.status = status


class Registry:
    def __init__(self, status):
        self.status, self.requests = status, []

    def open(self, request, timeout):
        self.requests.append(request)
        if '/token?' in request.full_url:
            return Response(b'{"token":"bounded-registry-token"}')
        if self.status == 200:
            return Response()
        raise HTTPError(request.full_url, self.status, 'controlled', {}, None)


class ReleaseTests(unittest.TestCase):
    def test_latest_actions_check_cannot_be_replaced_by_stale_or_other_app(self):
        checks = [{'id': i, 'name': name, 'status': 'completed', 'conclusion': 'success', 'app': {'slug': 'github-actions'}} for i, name in enumerate(sorted(REQUIRED))]
        self.assertTrue(verify(checks))
        checks.append({'id': 100, 'name': 'quality', 'status': 'in_progress', 'conclusion': None, 'app': {'slug': 'github-actions'}})
        self.assertFalse(verify(checks))
        checks[-1]['app']['slug'] = 'untrusted-app'
        self.assertTrue(verify(checks))
        self.assertFalse(verify(checks[1:]))

    def test_only_confirmed_404_allows_new_tag_and_failures_remain_blocking(self):
        image = 'ghcr.io/lincolncyber-01/breaking-bytes'
        self.assertTrue(available(image, 'v0.1.0', 'owner', 'contract-only-token', Registry(404)))
        self.assertFalse(available(image, 'v0.1.0', 'owner', 'contract-only-token', Registry(200)))
        for code in [401, 403, 429, 500]:
            with self.assertRaises(HTTPError):
                available(image, 'v0.1.0', 'owner', 'contract-only-token', Registry(code))
        with self.assertRaises(ValueError):
            available('https://untrusted.invalid', 'v0.1.0', 'owner', 'contract-only-token', Registry(404))

    def test_helper_returns_secret_only_to_docker_and_config_does_not_store_it(self):
        token = 'contract-only-token-from-environment'
        environment = dict(os.environ, GITHUB_ACTOR='owner', GITHUB_TOKEN=token)
        helper = Path(__file__).with_name('registry_credentials.py')
        result = subprocess.run([sys.executable, str(helper), 'get'], input=b'ghcr.io\n', env=environment, capture_output=True, check=True, timeout=5)
        self.assertEqual(json.loads(result.stdout)['Secret'], token)
        for command, server in [('store', 'ghcr.io'), ('get', 'untrusted.invalid')]:
            rejected = subprocess.run([sys.executable, str(helper), command], input=server.encode(), env=environment, capture_output=True, timeout=5)
            self.assertNotEqual(rejected.returncode, 0)
            self.assertNotIn(token.encode(), rejected.stdout + rejected.stderr)
        with configured(environment) as configured_environment:
            root = Path(configured_environment['DOCKER_CONFIG'])
            self.assertNotIn(token, (root / 'config.json').read_text())
            self.assertTrue((root / 'docker-credential-breakingbytes').stat().st_mode & 0o100)
        self.assertFalse(root.exists())
