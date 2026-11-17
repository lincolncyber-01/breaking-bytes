# Breaking Bytes

Pipeline de notícias tech para Stories em Rust, conforme [blueprint](docs/blueprint.pdf).
Documentação e comentários em português do Brasil; identificadores em inglês.

Implementação incremental: **fase 1 aprovada no CI; fase 2 em validação operacional**.
O binário executa migrations, health checks, administração, coleta e agrupamento.
Ainda não gera ou publica Stories. O marco de 24 horas de coleta está pendente.
O modo real permanece sem readiness até a implementação dos clientes externos.

## Ambiente local

Requer Rust 1.100.0, Docker e cargo-nextest 0.9.152. Versões estão fixadas no repositório.
Gere um POSTGRES_PASSWORD hexadecimal e um ADMIN_TOKEN aleatório de pelo menos
32 bytes. Exporte DATABASE_URL para um Postgres 16 com pgvector 0.8.2.
Segredos são lidos somente do ambiente; TOML aceita referências env:NOME.

```sh
cargo run --locked --bin stories-tech -- --migrate
cargo run --locked --bin stories-tech
curl --fail http://127.0.0.1:8080/health/live
curl --fail http://127.0.0.1:8080/health/ready
```

Por padrão, a API escuta somente loopback e dry_run=true. Configure arquivos com
STORIES_CONFIG e STORIES_SOURCES; sobrescreva campos com STORIES__SECAO__CAMPO.
Os arquivos são validados antes de abrir conexões. Nenhum segredo é exibido em Debug.

A coleta fica desativada por padrão (`collection.enabled=false`). Em egress validado,
ative com `STORIES__COLLECTION__ENABLED=true`. O cliente recusa proxies de ambiente
porque não pode assegurar a resolução DNS da conexão por esse caminho. Não altere
o proxy da Cloud para contornar essa verificação; valide uma VPS/rota apropriada.
RSS/Atom, sitemap, GitHub Releases, SEC Atom, HN e Statuspage são suportados.
SEC exige contato em `STORIES__COLLECTION__USER_AGENT`. Sem data de publicação,
o item não entra no pipeline; data de atualização não substitui publicação.

O agrupamento usa o fallback `hash_entities` do blueprint. Ele não demonstra
deduplicação semântica de paráfrases. Leia as [decisões de agrupamento](docs/adr/005-deduplicacao.md)
e [coleta](docs/adr/006-coleta.md) antes de habilitar fontes. Jobs de verificação
aguardam a fase 3. Nenhum cliente de publicação está ativo.


Para Compose, copie `.env.example` para `.env`, preencha os valores e use host
`postgres` em DATABASE_URL. O serviço de migrations termina antes de iniciar app:

```sh
docker compose --env-file .env -f deploy/docker-compose.yml up --build -d
```

## API administrativa

GET /health/live indica processo vivo. GET /health/ready sonda Postgres e declara
escopo foundation. Em modo real a resposta é 503 nesta fase.
GET /admin/status e PUT /admin/pause exigem Authorization: Bearer com ADMIN_TOKEN.
O status informa fase, coleta habilitada e modo de agrupamento.
O corpo do PUT é `{"paused":true}` ou `{"paused":false}`. A pausa é persistida
com auditoria. Não coloque o token na URL ou em comandos registrados no histórico.
Requisições têm limite de corpo, concorrência e tempo.

## Verificação

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --locked
```

Os testes sobem Postgres/pgvector reais por testcontainers; Docker é obrigatório.
Incluem transações de outbox, concorrência SKIP LOCKED, recuperação de lease,
fencing, retries, dead letter, unicidade de publicação e autenticação administrativa.
Os testes de propriedade verificam invariantes do domínio.

Consulte [ADRs](docs/adr/001-fundacao.md), [decisão Meta](docs/adr/002-meta-publicacao.md)
e os relatórios das [fases 1](docs/reports/phase-1.md) e [2](docs/reports/phase-2.md).
Conta Meta, R2,
avaliação editorial, snapshots, benchmarks e canário só serão declarados aprovados
com evidências específicas. Não existe implantação de produção nesta entrega parcial.
