# Matriz de rastreabilidade incremental

Estado em 17/11/2026. Esta matriz registra o que existe nas fases 1 a 4.

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
| Cobertura de domínio >=80% | cargo-llvm-cov | Core, normalize, cluster, verify, editorial e imaging: 92,03% de linhas na fase 4 |
| CI remoto verde | .github/workflows/ci.yml | Fases 1 e 2 aprovadas; fase 3 nos Checks da PR |
| Container mínimo e não root | deploy/Dockerfile, Compose | Build da fase 1 aprovado no CI |
| Feeds, SSRF, parsers, clusters | crates/collectors, normalize, cluster; store/collection | Contratos, propriedades e Postgres; embeddings/WebSub pendentes |
| Coleta real por 24 horas | Fase 2 | Não executada |
| Verificação N1/N2/N3 | crates/verify, collectors::confirm, store/editorial | Datas, origens, sinais e promoção N3 em testes/propriedades/Postgres |
| Redação e contrato LLM | crates/editorial, app/editorial | Schema, prompts separados e contratos HTTP simulados |
| Reescritas e outbox de render | migration 0003, store/editorial | Limite de duas, fencing, versões concorrentes e CHECKED+outbox atômicos |
| Fidelidade e prompt injection | Editorial, fixtures/evaluation.json, --evaluate-editorial | Rejeições locais e separação de prompts testadas; fidelidade real e revisão humana pendentes |
| Imagens, Inter, render, snapshots | crates/imaging, assets/fonts, tests/golden | MIME, pixels, EXIF, licenças e seis referências sem diferença; detecção visual automática substituída por revisão humana (ADR-009) |
| Artefato durável e outbox schedule | app/imaging, store/imaging, migration 0004 | Postgres real: fencing, rollback, retomada e invalidação após render |
| Benchmark de render e memória | imaging/benches, examples/benchmark | p95 76,5 ms / 85,2 ms em 16 MP; pico 159 MiB; máximo 413,8 ms; escopo CPU isolado |
| Detecção e memória da aplicação completa | Fases 2 e 6 | Coleta real, rede, LLM e carga completa ainda não medidos |
| R2, Meta, reconciliação e caos | Fase 5 | Não implementado; ADR-002 registra limites |
| Métricas, alertas e backups | Fase 6 | Não implementado |
| Audit, deny, fuzz e Trivy | Fase 6 | Ainda não configurados/executados |
| Dry-run completo por 48 horas | Fase 6 | Não executado |
| Canário por 3 dias | Fase 7 | Não executado |
