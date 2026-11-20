# Fase 7 — Entrega e lançamento controlado

Data: 20/11/2026. A implementação desta fase prepara o lançamento; não atesta
canário real nem substitui as contas e janelas temporais do proprietário.

## Plano e arquivos

Implementar release sem sobrescrever versão, deploy por digest com pausa,
migração e readiness, reversão conservadora, staging separado e observação das
janelas do blueprint. Registrar os resultados efetivamente executados e manter
os aceites externos explícitos.

```text
.github/CODEOWNERS
.github/actionlint.yaml
.github/workflows/release.yml
.github/workflows/deploy.yml
.github/workflows/security.yml
deploy/scripts/runtime.py
deploy/scripts/deploy.py
deploy/scripts/staging.py
deploy/scripts/control.py
deploy/scripts/acceptance.py
deploy/scripts/sql_report.py
deploy/scripts/test_acceptance.py
deploy/scripts/test_admin_contract.py
deploy/scripts/test_deploy.py
deploy/scripts/test_staging.py
deploy/scripts/test_sql_integration.py
deploy/sql/acceptance.sql
deploy/staging-compose.yml
deploy/systemd/breaking-bytes-acceptance.service
tools/check_release_checks.py
tools/docker_registry.py
tools/registry_credentials.py
tools/registry_tag.py
tools/test_release.py
crates/imaging/tests/snapshots/
docs/adr/019-lancamento-e-evidencias.md
docs/launch.md
docs/requirements.md
docs/reports/final.md
```

Também foram alterados Cargo.toml/lock, dev-dependencies do imaging, teste
visual, identificação da fase no app, Compose, catálogo de ambiente, README e
runbook. Não houve alteração de pixels das seis referências visuais.

## Implementação

Release é manual em main e exige os sete checks do SHA, versão exata e ausência
confirmada das tags; autenticação no GHCR usa helper sem arquivo de token. As
imagens próprias passam por SBOM, auditoria e scan antes do push. O artefato
release.json conserva revisão e digests de app/backup.

Deploy conserva snapshots não secretos, lock exclusivo e identidades Meta/R2;
confirma arquitetura e revisão da imagem. Pausa auditada precede a parada do
worker anterior, a migração e o readiness da nova imagem. Falha restaura o
anterior sob pausa; rollback explícito não migra nem restaura banco e recusa
retomada automática. Falha de pausa mantém os workers parados para intervenção.

Staging tem bootstrap e projeto/volume/porta/credenciais próprios. Publicação
exige flag explícita e conta numérica distinta de produção. O observador exige
janelas reais completas de 24/48/72 horas e fluxo, amostra processo/fontes/API e
recusa reinício, mudança de teto ou configuração. Consulta SQL é somente
leitura e limitada por timeout. Recibo vincula hashes das evidências e revisão
humana, sem distribuir aprovação fictícia. Promoção segue 10→20→40→80, com nova
janela em cada patamar. A disponibilidade de lançamento não comprova a mensal.

Insta 1.49.0 acrescenta snapshots estruturados do layout aos testes existentes
de pixels. As referências foram derivadas dos JSONs golden já versionados;
qualquer divergência desses valores também falha antes do snapshot.

## Verificação e riscos

Resultados locais: compilação completa em 6 min 16 s e recompilação da correção
do teste em 30,25 s; 111 testes Rust aprovados, zero ignorados, em 150,511 s;
12 testes operacionais aprovados em 9,670 s; nove testes de backup aprovados em
14,499 s e nove de ferramentas em 0,874 s. Clippy workspace/all-targets/
all-features/locked terminou sem avisos em 3 min 56 s, com warnings negados.
A formatação e actionlint terminaram com exit code 0. As saídas são preservadas:

| Verificação | Saída real |
| --- | --- |
| Compilação | [build](evidence/phase-7-build.txt) e [recompilação](evidence/phase-7-rebuild.txt) |
| Rust/nextest | [111 aprovados](evidence/phase-7-nextest.txt) |
| Clippy -D warnings | [log completo](evidence/phase-7-clippy.txt) |
| Operação/psql/PG | [12 aprovados](evidence/phase-7-operations.txt) |
| Backup/restore | [nove aprovados](evidence/phase-7-backup.txt) |
| Ferramentas/release | [nove aprovados](evidence/phase-7-tools.txt) |
| Fmt/workflows | [códigos e saídas](evidence/phase-7-static-checks.json) |
| Versão estável oficial | [manifesto consultado e toolchain](evidence/phase-7-toolchain.json) |

```text
Finished `dev` profile [unoptimized] target(s) in 3m 56s
Summary [150.511s] 111 tests run: 111 passed, 0 skipped
Ran 12 tests in 9.670s — OK
Ran 9 tests in 14.499s — OK
Ran 9 tests in 0.874s — OK
```

Actionlint foi executado sem os verificadores opcionais ShellCheck/Pyflakes;
não se atribui resultado a essas ferramentas. Compose de produção e staging
também foram validados sem imprimir ambientes resolvidos.

O novo teste revelou a promoção decimal de f32 ao usar serde_json::to_value:
60,8 virava 60,799999. A comparação agora usa o JSON serializado, como as
referências originais. O teste corrigido e toda a suíte passaram; não se
regeneraram pixels para aceitar a diferença.

Devido ao disco de 8 GB deste ambiente, a execução local comprimiu bibliotecas
geradas após o build e usou metadados dos 30 binários recém-compilados no
nextest, em série. Nenhum teste foi filtrado ou ignorado. O CI executa os
comandos normais locked, incluindo cargo nextest run com todos os recursos.
Os resultados completos são consolidados no [relatório de entrega](final.md).
A reconsulta das quatro
páginas oficiais Meta em 20/11/2026 confirmou permissões e fluxo e encontrou
novamente textos conflitantes de 100 e 50 publicações. O runtime consulta a cota
da conta; [ADR-002](../adr/002-meta-publicacao.md).

Os testes de lógica temporal usam relógio injetado e não contam como dias reais.
Rollout/falhas usam contrato de comandos; relatório SQL, read-only e pausa
auditada usam PostgreSQL/psql reais. Os workflows de release/deploy, o runner da
VPS, GHCR autenticado e serviços comerciais não foram acionados nesta entrega.
Proteção de main/environments exige configuração do proprietário. Alterações
incompatíveis de schema inviabilizam rollback automático; migrations futuras
devem conservar compatibilidade. O recibo é atestado local, não assinatura de
terceiro. Credenciais, corpus real e aceite de produção continuam pendentes.
