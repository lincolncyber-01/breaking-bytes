# Relatório da fase 3 — Verificação e redação

Estado: implementação validada localmente; aceite de fidelidade real pendente.
A fase 2 também conserva o marco pendente de 24 horas de coleta real.

## Plano executado e arquivos

O plano foi implementar níveis conservadores, confirmar conteúdo na origem,
isolar redação e auditoria, persistir retomada/reescritas e testar o fluxo com
Postgres e provedores simulados. A implementação parte da branch da fase 2,
ainda em revisão, sem integrar alterações à main automaticamente.

```text
crates/verify/
  Cargo.toml
  src/lib.rs
  tests/eligibility.rs
crates/editorial/
  Cargo.toml
  src/lib.rs
  src/schema.rs
  src/client.rs
  src/client/tests.rs
  tests/validation.rs
  tests/fixtures/evaluation.json
crates/app/src/
  editorial.rs
  editorial/tests.rs
  evaluation.rs
  pipeline.rs
crates/store/src/editorial.rs
migrations/0003_editorial.sql
docs/adr/007-editorial.md
docs/adr/008-contrato-anthropic.md
docs/reports/phase-3.md
```

Também foram atualizados Cargo.toml/lock, manifests existentes, app/collection,
app/main/lib/admin tests, collectors HTTP/contratos, normalize, config/testes,
store/lib, exemplos de ambiente/configuração/fontes, CI, README, runbook e matriz.
As evidências reais ficam em `docs/reports/evidence/phase-3-*`.

## Implementação

N1 requer fonte primária/peso 5, material e data confirmados e atuais. N2 requer
domínios registráveis, origens editoriais e artigos diferentes, sem trechos
idênticos. Sinais, rumores, datas desconhecidas e fontes insuficientes ficam N3.
A nova leitura da página/API confirma o conteúdo; trechos de API recebem a
mesma sanitização e limite de 2000 caracteres usados na coleta; updated/lastmod não renovam a
data original. A observação expira em max_age_hours ou avança quando confirmada.

O cliente Anthropic usa o contrato documentado, endpoint fixo, chave em header
sensível, timeout e limites de entrada/saída. Prompts de sistema são fixos;
matérias ficam nos dados de usuário e ferramentas não são disponibilizadas.
Redação e auditoria são chamadas independentes. Schemas fechados e regras locais
controlam formato, atribuição, números, datas, versões e entidades. A auditoria
cobre todos os segmentos e exige afirmações sustentadas e citações literais.

O worker serial renova o lease enquanto aguarda HTTP/CPU, respeita pausa e sinal
de parada e conserva resultados intermediários no Postgres. Há uma tentativa
inicial e até duas reescritas persistentes, com motivo estruturado para a nova
tentativa. 429/5xx transitórios preservam o estado e aplicam backoff/Retry-After.
Recusa, esgotamento editorial e falhas permanentes têm estados e razões explícitos.

Confirmações de banco verificam token, prazo do lease, versão editorial e revisão
das evidências. Novas associações, correções de matéria e mudanças de fonte
invalidam respostas em voo. O evento antigo vira SUPERSEDED e ganha substituto
CLUSTERED quando já havia sido verificado. CHECKED, auditoria e outbox render
são atômicos. O renderizador ainda será implementado na fase 4.

`--evaluate-editorial` executa a checagem de candidatos rotulados sem Postgres ou
publicação. O relatório separa rejeição factual, falsa aceitação e erro de
provedor. O corpus de oito casos sintéticos inclui negação, inversão de relação,
número/nome/data inventados, opinião e instrução maliciosa como dado.

## Verificado de fato

Os logs integrais registram os comandos e resultados locais:

```text
cargo fmt --check
exit_code: 0
cargo clippy --workspace --all-targets --locked -- -D warnings
Finished `dev` profile [unoptimized] target(s) in 3.38s
cargo nextest run --workspace --locked --test-threads 1
Summary [  31.012s] 46 tests run: 46 passed, 0 skipped
cargo llvm-cov --package stories-core --package stories-normalize --package stories-cluster --package stories-verify --package stories-editorial --locked --fail-under-lines 80 --summary-only
TOTAL: 760 linhas, 68 não cobertas, 91.05%
```

 Formatação passou,
Clippy passou com `-D warnings` e nextest executou 46 testes, todos aprovados,
sem testes ignorados. A cobertura local agregada de core/normalize/cluster/verify/
editorial foi **91,05% de linhas** (760 linhas, 68 não cobertas), acima do mínimo
de 80%. O relatório completo está em evidence/phase-3-coverage.txt.
A execução usa Postgres 16/pgvector reais via testcontainers,
com a imagem compactada equivalente descrita no ADR-004.

Foram exercitados:

- N1/N2/N3, idade/data futura, sinal, rumor, domínio registrável, origem editorial,
  cópias repetidas e fronteira de repositórios GitHub.
- Contratos Messages API, header de versão, schema, ausência de ferramentas,
  separação entre sistema e artigo, 429/Retry-After, timeout, recusa, tamanho,
  truncamento e tentativa de ação na resposta. Releases longos confirmados pela
  API ficam limitados a 2000 caracteres e têm scripts removidos. Corpo/chave não aparecem no erro.
- Estilo, atribuição, número/versão, moeda/data/nome novo, JSON com campos extras,
  afirmações omitidas, citação inventada e falso apoio na auditoria simulada.
- Fluxo até CHECKED/outbox no Postgres, duas reescritas e rejeição na terceira
  saída inválida, retomada de VERIFIED após 429, observação/expiração/promoção N3,
  jobs duplicados sem novo rascunho, versões concorrentes e fencing por lease.
- Mudança de conteúdo durante verificação, criação de substituto e invalidação
  por alteração de origem editorial. Sincronização idêntica não muda revisão.
- Comando de avaliação sem credencial: falha com código 1 antes de chamar o
  provedor, sem exigir DSN e sem criar relatório de fidelidade fictício.

O CI inclui fmt, Clippy, nextest, release, Docker e cobertura de domínio de
core/normalize/cluster/verify/editorial com mínimo agregado de 80% de linhas.
O resultado remoto pertence ao SHA da branch e deve ser consultado nos Checks
da PR. A cobertura não representa os módulos SQL ou a qualidade semântica do LLM.

## Não verificado, decisões e riscos

- Não houve chamada a LLM real: não há chave configurada. O comando de avaliação
  foi testado com provedor indisponível e os contratos com servidores simulados.
  Não há percentual de fidelidade real, medição de custo/cota ou latência da API.
- O blueprint não define percentual nem corpus editorial. ADR-007 propõe zero
  falsas aceitações e 100% de concordância em corpus rotulado pelo operador,
  incluindo revisão humana e amostras de geração livre. Os exemplos sintéticos
  não satisfazem esse marco operacional.
- Independência de agência/grupo e nomes próprios ambíguos não são demonstráveis
  apenas por hash, PSL e padrões lexicais. Há confirmação conservadora, auditoria
  independente e requisitos de revisão; não se promete detecção semântica perfeita.
- Páginas sem canonical/data explícita compatível podem ficar em N3. Isso reduz
  cobertura e exige adaptar uma fonte com evidência documental antes de liberá-la.
- Fontes reais, coleta por 24 horas, fidelidade com a conta do provedor e egress
  precisam de validação operacional. Não removemos o proxy gerenciado da Cloud.
- Snapshots, render abaixo de 150 ms e Story de exemplo pertencem à fase 4.
  Publicação Meta, reconciliação e caos pertencem à fase 5 e não foram executados.
- Render jobs aguardam consumidor; a aplicação não produz imagem nem publica.
  Readiness permanece restrita a dry_run e declara o escopo habilitado.

As atualizações explícitas de API e as suposições editoriais estão nos ADRs 007/008.
O próximo passo é a fase 4: validação de imagem, template com Inter, renderização,
snapshots e benchmark, mantendo os marcos operacionais pendentes visíveis.
