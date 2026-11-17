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
crates/store/tests/telemetry.rs
crates/app/src/{lib,main,pipeline,editorial,revisions,imaging,publication,observability}.rs
crates/app/src/observability/tests.rs
crates/app/tests/admin.rs
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
