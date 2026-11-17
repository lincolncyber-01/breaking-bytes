#!/usr/bin/env python3
"""Impede ampliar exceções de manutenção para CVEs, versões novas ou prazo vencido."""
import datetime as dt
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def validate(entry, document, locked_versions, today):
    advisory = document.get("advisory", {})
    if today > dt.date.fromisoformat(entry["expires"]):
        raise ValueError("excecao_vencida")
    if entry["kind"] != "unmaintained" or advisory.get("informational") != "unmaintained":
        raise ValueError("excecao_nao_pode_ignorar_vulnerabilidade")
    if advisory.get("id") != entry["id"] or advisory.get("package") != entry["package"]:
        raise ValueError("identidade_de_advisory_divergente")
    if any(alias.startswith("CVE-") for alias in advisory.get("aliases", [])):
        raise ValueError("cve_nao_pode_ser_ignorado")
    if document.get("versions", {}).get("patched"):
        raise ValueError("versao_corrigida_disponivel")
    if set(locked_versions) != set(entry["versions"]):
        raise ValueError("versao_do_lock_exige_nova_revisao")


def main():
    try:
        entries = json.loads((ROOT / "security/maintenance-exceptions.json").read_text())
        if not isinstance(entries, list) or not 1 <= len(entries) <= 3:
            raise ValueError("quantidade_de_excecoes_invalida")
        ids = {entry["id"] for entry in entries}
        if len(ids) != len(entries):
            raise ValueError("excecao_repetida")
        deny = tomllib.loads((ROOT / "deny.toml").read_text())
        audit = tomllib.loads((ROOT / ".cargo/audit.toml").read_text())
        deny_ids = {item if isinstance(item, str) else item["id"] for item in deny["advisories"]["ignore"]}
        if ids != deny_ids or ids != set(audit["advisories"]["ignore"]):
            raise ValueError("listas_de_excecao_divergentes")
        lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
        runtime = subprocess.run(["cargo", "tree", "--locked", "--offline", "--package", "stories-app", "--edges", "normal,build", "--prefix", "none", "--format", "{p}"], cwd=ROOT, capture_output=True, text=True, timeout=30, check=True).stdout
        for entry in entries:
            if not re.fullmatch(r"RUSTSEC-[0-9]{4}-[0-9]{4}", entry["id"]) or not re.fullmatch(r"[a-z0-9_-]{1,64}", entry["package"]):
                raise ValueError("identificador_invalido")
            if entry["dev_only"] and re.search(r"^" + re.escape(entry["package"]) + r" v", runtime, re.MULTILINE):
                raise ValueError("dependencia_de_teste_entrou_no_runtime")
            url = f"https://raw.githubusercontent.com/RustSec/advisory-db/main/crates/{entry['package']}/{entry['id']}.md"
            with urllib.request.urlopen(url, timeout=15) as response:
                text = response.read(65537)
            if len(text) > 65536:
                raise ValueError("advisory_acima_do_limite")
            document = tomllib.loads(text.decode().split("```toml", 1)[1].split("```", 1)[0])
            versions = [p["version"] for p in lock["package"] if p["name"] == entry["package"]]
            validate(entry, document, versions, dt.datetime.now(dt.timezone.utc).date())
        print(json.dumps({"status": "excecoes_informativas_verificadas", "count": len(entries), "expires": min(e["expires"] for e in entries)}))
        return 0
    except (ValueError, KeyError, IndexError, TypeError, OSError, subprocess.SubprocessError):
        print("Revisão de exceções de manutenção falhou; conferir ADR-017, prazo, versões e fonte RustSec.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
