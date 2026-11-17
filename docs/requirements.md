# Matriz de rastreabilidade incremental

Estado em 16/11/2026. Esta matriz registra somente o que existe na fase 1.

| Requisito do blueprint | Implementação | Verificação / estado |
| --- | --- | --- |
| Monólito modular, um binário | Cargo.toml, crates/app | Clippy e build nativo |
| Rust estável, edição atual, lock | rust-toolchain.toml, Cargo.lock | Rust 1.100.0, edição 2024 |
| Estados de evento | crates/core, trigger events_state_guard | Unitários, propriedades e Postgres |
| Modelo de dados e vector(384) | migrations/0001_foundation.sql | Migrations repetidas e extensão real |
| Outbox transacional | Store::create_event, Store::transition | Rollback de evento quando job conflita |
| Lease, SKIP LOCKED, retomada | Store::claim, complete, renew | Claims concorrentes, recuperação e fencing |
| Backoff com jitter e dead letter | Store::retry, retry_delay | Adiamento real e esgotamento de tentativas |
| Chave única de publicação | PublicationKey, publications | Reservas concorrentes no Postgres |
| Config TOML e ambiente | crates/config | Limites, desconhecidos, segredos e dry_run |
| Segredos fora dos logs | Secret, erros sanitizados | Debug e logs reais do processo |
| Liveness e readiness | crates/app | Router e binário real por TCP |
| Admin protegida e kill switch | /admin/status, /admin/pause | Autenticação, corpo limitado e pausa persistida |
| Shutdown gracioso | app::main | Processo recebe SIGTERM e encerra com sucesso |
| Formatação, Clippy, nextest | CI e comandos locais | Resultados em reports/phase-1.md |
| Cobertura de domínio >=80% | cargo-llvm-cov | 100% de linhas na medição atual |
| CI remoto verde | .github/workflows/ci.yml | Código enviado; pendente de conclusão do CI |
| Container mínimo e não root | deploy/Dockerfile, Compose | Build Docker bloqueado por disco; pendente |
| Feeds, SSRF, parsers, clusters | Fase 2 | Não implementado |
| Coleta real por 24 horas | Fase 2 | Não executada |
| Verificação N1/N2/N3 e LLM | Fase 3 | Não implementado |
| Fidelidade e prompt injection | Fase 3 | Não implementado |
| Imagens, Inter, render, snapshots | Fase 4 | Não implementado |
| Benchmark de render e memória | Fases 4 e 6 | Não executado |
| R2, Meta, reconciliação e caos | Fase 5 | Não implementado; ADR-002 registra limites |
| Métricas, alertas e backups | Fase 6 | Não implementado |
| Audit, deny, fuzz e Trivy | Fase 6 | Ainda não configurados/executados |
| Dry-run completo por 48 horas | Fase 6 | Não executado |
| Canário por 3 dias | Fase 7 | Não executado |
