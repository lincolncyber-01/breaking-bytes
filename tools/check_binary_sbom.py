#!/usr/bin/env python3
"""Confere a lista de dependências embutida pelo cargo-auditable antes da auditoria."""
import json
from pathlib import Path
import sys
import tomllib
import zlib


def decode(data):
    if not data or len(data) > 1024 * 1024:
        raise ValueError("secao_invalida")
    decompressor = zlib.decompressobj()
    raw = decompressor.decompress(data, 8 * 1024 * 1024 + 1)
    if not decompressor.eof or len(raw) > 8 * 1024 * 1024:
        raise ValueError("sbom_acima_do_limite")
    value = json.loads(raw)
    packages = value.get("packages")
    if not isinstance(packages, list) or not 1 <= len(packages) <= 1000:
        raise ValueError("pacotes_invalidos")
    return value


def validate(value, locked):
    packages = value["packages"]
    roots = [p for p in packages if p.get("root")]
    if len(roots) != 1 or roots[0].get("name") != "stories-app":
        raise ValueError("raiz_incorreta")
    for package in packages:
        name, version = package.get("name"), package.get("version")
        if name in {"libsqlite3-sys", "sqlx-sqlite", "sqlx-mysql"}:
            raise ValueError("backend_nao_autorizado_no_binario")
        if (name, version) not in locked:
            raise ValueError("pacote_fora_do_lock")


def main():
    try:
        if len(sys.argv) != 3:
            raise ValueError("argumentos_invalidos")
        section, output = map(Path, sys.argv[1:])
        value = decode(section.read_bytes())
        root = Path(__file__).resolve().parents[1]
        lock = tomllib.loads((root / "Cargo.lock").read_text())
        validate(value, {(p["name"], p["version"]) for p in lock["package"]})
        output.write_text(json.dumps(value, indent=2) + "\n")
        print(json.dumps({"status": "sbom_verificada", "packages": len(value["packages"]), "format": value.get("format", 0)}))
        return 0
    except (ValueError, KeyError, TypeError, OSError, zlib.error):
        print("SBOM ausente, inválida ou divergente do produto Postgres configurado.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
