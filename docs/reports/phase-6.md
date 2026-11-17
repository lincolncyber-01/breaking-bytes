# Fase 6 — plano, implementação e evidências

O código desta fase sucede os 104 testes e Clippy sem avisos da fase 5.
Os aceites com serviços reais permanecem explicitamente pendentes.

Plano: métricas Prometheus com cardinalidade controlada e contadores duráveis;
alertas Telegram deduplicados com lease; painel Grafana; backup noturno R2 e
restauração isolada mensal; análise de dependências, imagem e fuzz; documentação
operacional e limites. O dump é ferramenta operacional separada do runtime Tokio.

Árvore criada ou alterada:

```text
crates/config/src/lib.rs
crates/config/tests/settings.rs
crates/store/src/{lib,collection,telemetry,security}.rs
crates/store/tests/{telemetry,collection}.rs
crates/app/src/{lib,main,pipeline,editorial,revisions,imaging,publication,observability}.rs
crates/app/src/observability/tests.rs
crates/app/tests/admin.rs
crates/collectors/src/{lib,tests}.rs
crates/app/src/{admin,token}/tests.rs
crates/publisher/src/{lib,graph}.rs
config/exemplo.toml
migrations/0009_observability.sql
deploy/{Dockerfile,docker-compose.yml,bootstrap-compose.yml,Caddyfile,caddy-compose.yml}
deploy/monitoring/{prometheus.yml,datasource.yml,dashboard-provider.yml,dashboard.json}
deploy/backup/{Dockerfile,backup.py,test_backup.py,compose.yml,restore-compose.yml}
deploy/systemd/{breaking-bytes-backup.service,breaking-bytes-backup.timer,breaking-bytes-restore.service,breaking-bytes-restore.timer}
.github/{dependabot.yml,workflows/ci.yml,workflows/security.yml,workflows/fuzz.yml}
deny.toml
fuzz/{Cargo.toml,Cargo.lock,fuzz_targets/feed.rs,fuzz_targets/html.rs,fuzz_targets/image.rs}
docs/{adr/016-operacao.md,backups.md,observability.md,runbook.md,reports/phase-6.md}
```

Os resultados efetivamente executados serão acrescentados após os portões.

## Resultados intermediários

Antes da integração do exporter: cargo check de todos os alvos/recursos passou em
10,25 s; Clippy sem avisos passou em 11,68 s. Com metrics 0.24.6 e exporter 0.18.3,
Clippy de todos os alvos/recursos passou em 3m24s. O patch UUID 1.29.3 e os alvos
fuzz precisam da execução final. Não se extrapola esse resultado para arquivos
modificados depois da execução.

O primeiro teste de backup/restauração executou oito testes em 14,696 s, nenhum
ignorado. pg_dump/pg_restore operaram em dois clusters Docker reais e sucessivos,
com IDs distintos; R2 foi armazenamento de contrato. A barreira SQL impediu retirar
a pausa após restaurar. A coleta de órfãos e os manifests estritos adicionados
posteriormente serão revalidados na execução final.

Os calendários systemd foram executados: backup em 18/11/2026 00h America/Bahia
(03h UTC); restauração em 01/12/2026 01h (04h UTC). Isso valida a expressão, não a
instalação de um serviço na VPS. Não houve chamada a Meta, R2, LLM ou Telegram reais.

## Portões locais atuais

Recompilação limpa: test profile concluído em 6m18s; Clippy de todos os alvos e
recursos terminou em 2m44s, sem avisos. A suíte integral passou:

```text
Summary [ 149.207s] 110 tests run: 110 passed, 0 skipped
```

Inclui contratos de Telegram, exporter real, papéis reais de banco, migração pelo
papel sem superuser, quarentena, leases, snapshots e SIGKILL. Backup/restauração:
9 testes em 13,658 s, nenhum ignorado, com dois clusters Postgres reais e R2 de
contrato. A seleção adicional de três benchmarks passou em 21,175 s; os demais
107 testes foram filtrados nessa medição e já haviam passado na suíte integral.

Medições locais em debug: captura HTTP/robots/parsing/normalização de 200 itens,
p95 de 82,134 ms por lote; rajada de 200 matérias equivalentes ingerida e agrupada
em 1,480 s, cluster p95 de 5,223 ms; RSS do processo de captura 20.576 KiB.
O binário real ocioso, após readiness e uma janela de um segundo, registrou RSS
38.808 KiB. Esse valor não representa memória sob carga de publicação real.
Essas medições não incluem polling, Internet, LLM, R2 ou Meta. O benchmark release
do renderizador e as medições release novas são artefatos do job de desempenho.

No primeiro CI do PR, fuzz dos três parsers, cobertura de domínio e contratos
operacionais passaram. A suíte Rust de 109 testes daquele commit também passou.
Os testes de benchmark locais elevam o total a 110. Os scans iniciais reprovaram
por três avisos de manutenção, licença MIT-0 não admitida e OpenSSL da base. A
correção e as exceções limitadas estão em ADR-017, com validação automática.
Os resultados desses portões ainda precisam da nova execução do código final.
