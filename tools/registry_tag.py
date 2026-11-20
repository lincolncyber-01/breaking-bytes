#!/usr/bin/env python3
"""Consulta OCI Distribution: somente 404 confirmado permite um novo tag de release."""
import base64
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_):
        return None


def available(image, version, actor, token, opener=None):
    if image not in {'ghcr.io/lincolncyber-01/breaking-bytes', 'ghcr.io/lincolncyber-01/breaking-bytes-backup'} or not re.fullmatch(r'v\d+\.\d+\.\d+', version):
        raise ValueError('referencia_invalida')
    if not token or not actor or any(c in token + actor for c in '\r\n\0'):
        raise ValueError('credencial_invalida')
    client = opener or urllib.request.build_opener(NoRedirect())
    name = image.removeprefix('ghcr.io/')
    query = urllib.parse.urlencode({'service': 'ghcr.io', 'scope': 'repository:' + name + ':pull'})
    authorization = base64.b64encode((actor + ':' + token).encode()).decode()
    request = urllib.request.Request('https://ghcr.io/token?' + query, headers={'Authorization': 'Basic ' + authorization})
    with client.open(request, timeout=10) as response:
        raw = response.read(65537)
        if len(raw) > 65536:
            raise ValueError('resposta_excessiva')
        result = json.loads(raw)
    bearer = result.get('token') or result.get('access_token')
    if not isinstance(bearer, str) or not bearer or any(c in bearer for c in '\r\n\0'):
        raise ValueError('credencial_de_registro_invalida')
    request = urllib.request.Request('https://ghcr.io/v2/' + name + '/manifests/' + version, method='HEAD',
        headers={'Authorization': 'Bearer ' + bearer, 'Accept': 'application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json'})
    try:
        with client.open(request, timeout=10) as response:
            if response.status == 200:
                return False
            raise ValueError('estado_do_registro_desconhecido')
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return True
        raise


def main():
    try:
        if len(sys.argv) != 3 or not available(sys.argv[1], sys.argv[2], os.environ.get('GITHUB_ACTOR', ''), os.environ.get('GITHUB_TOKEN', '')):
            raise ValueError('tag_existente_ou_invalido')
        print('Tag de release ainda ausente; publicação autorizada pelos demais gates.')
        return 0
    except (OSError, ValueError, urllib.error.URLError):
        print('Tag existente ou impossível de verificar; não será sobrescrito.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
