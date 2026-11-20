# Matriz de rastreabilidade

Estado técnico em 20/11/2026. “Contrato” significa API simulada; Postgres e
processos são reais. Aceites externos não foram inferidos de contratos.

| Requisito do blueprint | Onde foi implementado | Como foi verificado / limite |
| --- | --- | --- |
| Workspace exato, um binário, Rust atual | Cargo.toml, rust-toolchain.toml, crates/app | 12 crates, Rust 1.100.0/edição 2024, builds e Clippy |
| Versões fixas, APIs de crates | Cargo.lock e dependências exatas | Compilação locked; CI e ferramentas também fixos |
| Erros tipados e produção sem panic | thiserror em bibliotecas, anyhow no app, lints workspace | Clippy all-targets/all-features com warnings negados |
| Tokio, CPU fora do runtime, limites e timeouts | collectors, imaging, app workers | Spawn blocking, semáforos/canais limitados; contratos de timeout/tamanho |
| Modelo Postgres/pgvector e migrations | migrations/0001–0009, store | Banco real, migrations repetidas, owner sem superuser |
| Máquina de estados e motivo terminal | core, triggers SQL | Propriedades e integração Postgres |
| Outbox, lease, SKIP LOCKED e fencing | store/lib.rs e módulos | Claims concorrentes, rollback, queda, lease vencido |
| Retry/backoff/jitter/dead letter | store, collectors, publisher | Propriedades, Retry-After e retomadas reais |
| Camadas A/B/C/D e confirmação | config/sources.toml, collectors, verify | RSS/Atom, sitemap, GitHub, SEC, HN, Statuspage por contrato |
| Robots, datas reais, canonicals e validadores | collectors/http.rs, normalize, store/collection | XML/HTML/HTTP simulados, SSRF, transação do cursor |
| Coleta 24 h real sem duplicatas | deploy/scripts/acceptance.py, docs/launch.md | Regras temporais e fluxo testados; janela real e revisão semântica pendentes |
| WebSub e demais redes sociais | ADR-006 | WebSub não implementado; polling é fallback; Reddit/X/Bluesky e agências exigem integração/licença |
| Deduplicação URL/hash/entidades | normalize, cluster, store/collection | Propriedades, concorrência e rajada de 200 itens |
| Embeddings locais 384 dimensões | vector(384) no schema, ADR-005 | Backend fastembed não implementado; fallback explícito, não prova paráfrases |
| N1/N2/N3 e origens independentes | verify, store/editorial | Propriedades, fonte primária, rumores, datas e N3 promovido em Postgres |
| JSON editorial, prompt como dado, PT-BR | editorial, schemas/prompts | Schema/determinístico/contratos; corpus sintético de 8 casos |
| Segunda checagem e duas reescritas | app/editorial, store/editorial | Rejeições, fencing/revisão e orçamento durável |
| Fidelidade real zero afirmações sem fonte | --evaluate-editorial e runbook | Chave/modelo/corpus real/revisão humana pendentes |
| Imagem: licença, MIME, bytes e pixels | imaging, config/images.json | Decodificação, EXIF, decompression bomb, manifesto vinculado ao hash |
| Marca d'água/relevância | ADR-009 e manifesto revisado | Revisão humana conservadora; detector automático não implementado |
| Template visual exato e Inter | imaging, assets/fonts, tests/golden e snapshots | Seis golden files pixel a pixel e Insta 1.49.0 do layout; JPEG de exemplo versionado |
| Texto sem corte e redução <=10% | imaging/layout e app/imaging | Limites e reescrita com contador global |
| Artefato durável/revisão/outbox | store/imaging, app/imaging | Arquivo sincronizado, fencing, rollback e invalidação |
| Render <150 ms e cluster <50 ms | benches, examples/benchmark, testes de rajada | Release CI fase 6: render p95 53,957/81,173 ms; cluster p95 3,031 ms |
| Detecção <2 min/publicação <5 min | Métricas, deploy/sql/acceptance.sql e observador | Captura de contrato p95 4,930 ms/lote; SLO real pendente; SQL exige amostras |
| Memória normal <300 MB | Limite Compose, métricas RSS e benches | Render isolado pico ~161 MiB; app debug ocioso 38.808 KiB; carga completa pendente |
| Prioridade, cadência e teto móvel | scheduler, store/scheduling, publishing | Propriedades/banco, pesos configuráveis, cota atual e reservas |
| Calibração impacto/alcance/urgência | ADR-011 | Sinais conservadores explícitos; calibração real pendente |
| Instagram Stories/cota/permissões | publisher/graph, ADR-002 | Documentação Meta oficial e contrato HTTP; cota/documentação divergentes, runtime consulta conta |
| R2 S3 e URL curta/limpeza | publisher/storage, app/publication, ADR-011 | SigV4 e objetos por contrato; R2 real pendente |
| Intenção única e reconciliação | store/publication, publisher, ADR-010/015 | Postgres, barrier-before-POST, retomada e resolução atestada |
| Caos: queda entre publish e commit | app/tests/publication_chaos.rs e testes operacionais | SIGKILL em processo real; um POST/efeito na API simulada; intenção preservada |
| Zero duplicatas/perdas remotas | Protocolo conservador, ADR-010 | API não dá transação distribuída nem correlação suficiente; ambiguidades bloqueadas |
| Token válido/renovação/alerta | app/token, publisher/token, ADR-012/013 | Facebook inspection, recibo Instagram Login, contratos; rotação externa manual |
| Reconsulta e correções 24/72 h | app/revisions, store/revisions, ADR-014 | Baseline congelado, revisão, quotes, nova outbox; contrato/PG |
| Kill switch, blocklist e resolução | app/admin, store/operations | Bearer, pausa, concorrência, recibo UUID e auditoria |
| Logs/segredos/SSRF/prompt injection | core::Secret, collectors/http, editorial | Logs do processo, corpos remotos sanitizados, testes de redes e dados hostis |
| Menor privilégio e auditoria íntegra | store/security, migration 0009 | Papéis runtime/backup/owner reais; runtime sem DDL/alterar auditoria |
| Métricas/painel/alertas | app/observability, store/telemetry, deploy/monitoring | Exporter e Telegram de contrato; disparo Telegram real pendente |
| Backup noturno 7 dias/4 semanas | deploy/backup e systemd | Retenção, limites, checksums PUT/GET; R2 simulado |
| Restauração mensal isolada | backup.py restore-check e restore-compose | Dois clusters reais, identidade distinta, dados e hold de 24 h |
| Testes/coverage/fuzz | .github/workflows/ci.yml e fuzz.yml | 111 Rust; cobertura fase 6 91,98% do domínio; 3 parsers 20 s/alvo; evidências por commit nos relatórios |
| Supply chain e imagens | security.yml, deny.toml, tools, ADR-017/018 | Gates reais, inventário do binário e Trivy; resultados finais no relatório da fase 6 |
| Docker/Compose/proxy/logs/limites | deploy/* | CI constrói/renderiza sem rede, valida Compose/Caddy/Prometheus/calendários |
| CI/CD versionado, rollback e canário | release.yml, deploy.yml, deploy/scripts e integration/rollout.py, ADR-019/020 | Contratos e ensaio Compose/PG: troca de imagem, migração, readiness, dados e pausa preservados; registry/VPS reais pendentes; relatório complementar da fase 7 |
| Dev/staging/prod, CODEOWNERS e revisão | staging-compose.yml, scripts/staging.py, .github/CODEOWNERS, docs/launch.md | Isolamento/recusa de conta igual testados; environments/proteção de main são configuração manual |
| Disponibilidade 99,5% mensal | Health, restart, métricas e runbook | Exige janela mensal; não demonstrada por teste curto |
| Dry-run 48 h e canário 72 h/10 por dia | acceptance.py, deploy.py e docs/launch.md | Duração completa, integridade, versão, relatório SQL e revisão exigidos; testes de lógica não equivalem às janelas reais |
