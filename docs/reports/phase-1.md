# Relatório da fase 1 — Fundação

Data: 16/11/2026. Repositório de destino: lincolncyber-01/breaking-bytes.
Estado: implementação e portões locais/remotos aprovados; integrada à main.

A execução remota confirmou testes, cobertura, release e Docker:
https://github.com/lincolncyber-01/breaking-bytes/actions/runs/43990961832
Os registros abaixo preservam os resultados locais e as pendências daquele momento.

## O que foi implementado

Workspace incremental com os crates core, config, store e app, sem módulos vazios.
Rust 1.100.0, edição 2024, dependências diretas exatas e Cargo.lock. Um binário
stories-tech, perfis release com LTO fat, codegen-units=1, panic abort e strip.

Máquina de estados completa do blueprint, motivos obrigatórios nos ramos terminais
e chave de publicação tipada. Configuração TOML e ambiente via Figment, limites
validados, rejeição de campos desconhecidos e segredos referenciados por ambiente.

Schema com todas as tabelas do blueprint, extensão vector e índices. Guardas de
estado no banco. Transações confirmam estado, auditoria e próximo job juntos.
Jobs usam SKIP LOCKED, expiração e token de lease, fencing de trabalhador antigo,
backoff exponencial com jitter, deduplicação e dead letter. A intenção de publicação
tem restrição única por evento, tipo e sequência; isso não garante sozinha
exatamente uma vez na API remota.

Admin API com liveness, readiness de escopo foundation, autenticação Bearer,
comparação de token em tempo constante, limites de corpo, concorrência e timeout.
Pausa global persistida com auditoria. Logs JSON, sem DSN ou credenciais,
shutdown gracioso e comandos --migrate e --healthcheck.

Dockerfile multi-stage, runtime não root, Compose com ordem de migrations,
limites, volume persistente, logs com rotação, filesystem read-only e capabilities
removidas. CI de formatação, Clippy, nextest, cobertura, release e build da imagem.
README, runbook, matriz de requisitos, quatro ADRs e PDF original versionáveis.

## Verificado de fato

Comandos e saídas integrais estão em evidence/. As últimas execuções retornaram:

```text
cargo fmt --check
exit_code: 0

cargo clippy --workspace --all-targets --locked -- -D warnings
Finished `dev` profile [unoptimized] target(s) in 1.11s

cargo nextest run --workspace --locked --test-threads 1
Summary [  30.601s] 15 tests run: 15 passed, 0 skipped

cargo llvm-cov --package stories-core --locked --fail-under-lines 80 --summary-only
TOTAL: 69 linhas, 0 linhas não cobertas, 100.00%

cargo build --release --locked --package stories-app --bin stories-tech
Finished `release` profile [optimized] target(s) in 2m 26s
```

A medição de cobertura cobre apenas lógica de domínio existente em crates/core.
Não representa cobertura do sistema completo. Não há medição de branches nesta execução.

Testes de Postgres usam testcontainers e banco real com pgvector 0.8.2. Foram
verificados migrations idempotentes, rollback da outbox, concorrência de claims,
recuperação de lease vencido, rejeição do trabalhador antigo, backoff,
dead letter e reservas concorrentes de publicação. A pausa e sua auditoria
são persistidas. O trigger bloqueia saltos de estado feitos por SQL direto.

O teste da aplicação também executa o binário real: aplica migrations, inicia
HTTP, aguarda readiness por TCP, executa --healthcheck, envia SIGTERM e exige
encerramento bem-sucedido. As linhas de log são JSON válido e não contêm
o token administrativo, a senha de teste ou o DSN.

A execução local selecionou STORIES_TEST_POSTGRES_IMAGE=breaking-bytes/pgvector-fixture.
É uma compactação local da imagem oficial para caber no Docker VFS. Não é um mock;
o CI mantém a imagem oficial padrão. Ver ADR-004.

## O que não foi verificado

- CI no GitHub: pendente de conclusão. O bloqueio inicial HTTP 403 foi resolvido
  ao selecionar a conexão do proprietário lincolncyber-01. Os 43 arquivos foram
  enviados à branch codex/phase-1-foundation; a árvore remota
  bcf7afb826facff25e7add7521284b9ab08abb25 é idêntica à validada localmente.
- Build Docker completo: falhou ao registrar a camada da imagem Rust por falta
  de espaço no volume de 8 GB. Não foi declarado aprovado.
- Execução em VPS, probes de R2 e Meta e publicação real: sem credenciais e ainda
  fora da implementação da fase 1. Readiness em modo real retorna 503.
- Contratos de feed/Meta, snapshots, benchmark de render, carga de 200 itens,
  teste de caos, backups, alertas e canário: componentes das fases seguintes.
- Coleta por 24 h, dry-run completo por 48 h e canário por 3 dias: não executados.

## Decisões, suposições e riscos

ADRs 001–004 documentam a construção incremental do workspace, adições ao schema,
divergência de cota na documentação Meta, reconciliação sob resultado incerto,
modelo LLM confirmado documentalmente e limitação de armazenamento dos testes.
O login Facebook foi escolhido inicialmente pela documentação do endpoint stories.
As credenciais e permissões efetivas da conta ainda precisam de validação.

O modelo claude-haiku-5-5 foi confirmado na documentação oficial, sem chamada
autenticada. APIs dos crates usados nesta fase foram verificadas pela compilação.
As funções futuras da API Meta não foram simuladas em código nesta fase.

O pipeline de notícias está indisponível nesta entrega parcial, explicitamente
informado por /admin/status. dry_run apenas configura a fundação; o modo completo
de geração de Stories será implementado nas fases seguintes. Nenhuma publicação
real é possível com este código.

## Próximo passo

Observar o CI do PR com espaço suficiente para validar o build Docker.
A fase 1 só recebe aprovação completa quando esse portão passar. A próxima fase
de implementação é captura e deduplicação; seu marco de 24 horas continuará
dependendo de observação real.
