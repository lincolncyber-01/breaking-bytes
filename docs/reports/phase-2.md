# Relatório da fase 2 — Captura e deduplicação

Data: 16/11/2026. Estado: implementação validada localmente; aceite operacional pendente.

## Implementação

Foram adicionados collectors, normalize e cluster ao workspace. O cliente HTTP
compartilhado aplica timeout, limites, DNS público, controle de concorrência e
robots.txt. Parsing e normalização usam spawn_blocking e semáforos de CPU.
Há coletores RSS/Atom, sitemap, GitHub Releases, SEC Atom, HN e Statuspage.
Fontes sem data de publicação não são promovidas com base em updated ou lastmod.

URLs são canonicalizadas, títulos recebem hash BLAKE3 normalizado e os metadados
têm entidades, fatos e tokens determinísticos. HTML lido fornece canonical e
metadados. O agrupamento implementa explicitamente o fallback hash_entities.
Embeddings e avaliação semântica ainda estão pendentes, conforme ADR-005.

A migration 0002 acrescenta cursores HTTP, histórico de saúde corrente das fontes,
proveniência das URLs, campos normalizados e relações de atualização de eventos.
Lotes e cursores são transacionais. Cada artigo novo tem outbox de cluster;
agrupamento, auditoria, sinal de verificação e conclusão do job são atômicos.
Corridas de trabalhadores são serializadas por lock curto e protegidas por lease.
Artigos equivalentes de fontes diferentes preservam suas origens no banco.

O binário supervisiona coleta e cluster, aplica backpressure com canal limitado,
jitter, circuit breaker persistido e retomada após reinício. Pausa bloqueia a
aquisição de jobs no próprio SQL e novas requisições de coleta. Shutdown fecha a
entrada, conserva lotes já recebidos e conclui transações antes de fechar o pool.
Falhas transitórias de fonte não interrompem as outras fontes.

## Verificado de fato

Os arquivos em evidence/phase-2-* preservam as saídas integrais. Última execução:

```text
cargo fmt --check
exit_code: 0

cargo clippy --workspace --all-targets --locked -- -D warnings
Finished `dev` profile [unoptimized] target(s) in 2.73s

cargo nextest run --workspace --locked --test-threads 1
Summary [  23.892s] 31 tests run: 31 passed, 0 skipped

cargo llvm-cov --package stories-core --package stories-normalize --package stories-cluster --locked --fail-under-lines 80 --summary-only
TOTAL: 271 linhas, 14 não cobertas, 94.83%
```

Foram verificados contratos de RSS/Atom e JSON, 304, ETag, erros transitórios,
Retry-After, timeout, redirecionamentos, robots, limites XML/HTML e SSRF, incluindo
IPv4 numérico e IPv6. Propriedades cobrem canonicalização idempotente, estabilidade
do agrupamento sob formatação e similaridade simétrica e limitada.

Postgres/pgvector reais exercitam rollback no meio de um lote sem avançar o cursor,
proveniência, agrupamento concorrente, fonte principal, novos fatos após publicação,
retomada com fencing e pausa. Uma rajada de 200 artigos foi inserida, repetida e
processada em menos de 60 segundos sem duplicar URLs ou perder jobs. Isso não é
um benchmark Criterion nem prova deduplicação semântica universal.

A execução local usa a imagem Postgres compactada descrita no ADR-004. O CI mantém
a imagem oficial. A fase 1 já teve CI e Docker aprovados e foi integrada à main.
Os endpoints Rust Blog e Rust Releases retornaram HTTP 200 via curl e proxy;
isso não demonstra o caminho de rede do coletor Rust.

## Não verificado e desvios explícitos

- Coleta real contínua por 24 horas: não executada. O proxy da Cloud impede
  garantir DNS público na conexão; o coletor recusa essa configuração. A coleta
  fica desativada no exemplo até validar uma VPS/egress apropriado.
- Zero duplicatas semânticas e fastembed: não demonstrados. O fallback não
  substitui uma avaliação com paráfrases e modelo calibrado. A publicação segue
  indisponível.
- WebSub: adiado até existir callback TLS validado; polling é a alternativa.
- APIs autenticadas/licenciadas, SEC com contato real e condições de reutilização
  de cada nova fonte: precisam de configuração e validação do proprietário.
- Benchmark p95 de detecção, clustering e memória: ainda não medido. O teste de
  rajada apenas verifica o limite de carga definido na estratégia de testes.
- LLM, geração de Stories, snapshots e publicação real: fases seguintes.

## Próximos passos

Enviar a branch e observar o CI remoto, inclusive build release e Docker.
Em infraestrutura autorizada, aplicar migrations, configurar segredos por ambiente,
validar as fontes e ativar collection.enabled. Registrar 24 horas de logs e banco,
contagens de itens, erros, retomadas e revisão semântica dos clusters. O marco só
recebe aceite quando existir essa evidência; não foi marcado como concluído.

A próxima implementação é a fase 3: níveis de verificação, cliente LLM, saída
estruturada, checagem determinística e avaliação editorial. O relatório de aceite
final deve manter as pendências operacionais visíveis.
