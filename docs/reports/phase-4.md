# Relatório da fase 4 — Imagem e renderização

Estado: implementação entregue com validação local; CI do SHA deve ser conferido
nos Checks da PR. As pendências operacionais das fases 2 e 3 continuam abertas.

## Plano e arquivos

O plano foi criar imaging, confirmar limites e licenças antes do decode, renderizar
o template fixo, conservar referências visuais e conectar CHECKED a um JPEG
durável com outbox. A implementação parte de codex/phase-3-editorial.

```text
crates/imaging/
  Cargo.toml
  src/lib.rs
  src/assets.rs
  src/validate.rs
  src/render.rs
  tests/security.rs
  tests/visual.rs
  tests/fixtures/stories.json
  tests/golden/short.jpg
  tests/golden/short.json
  tests/golden/long.jpg
  tests/golden/long.json
  tests/golden/update.jpg
  tests/golden/update.json
  tests/golden/correction.jpg
  tests/golden/correction.json
  tests/golden/accents.jpg
  tests/golden/accents.json
  tests/golden/shrink.jpg
  tests/golden/shrink.json
  benches/story.rs
  examples/reference.rs
  examples/benchmark.rs
assets/fonts/
  Inter-Regular.otf
  Inter-Medium.otf
  Inter-SemiBold.otf
  LICENSE.txt
  provenance.json
config/images.json
crates/store/src/imaging.rs
crates/app/src/imaging.rs
crates/app/src/imaging/tests.rs
migrations/0004_imaging.sql
docs/adr/009-imagens-renderizacao.md
docs/images.md
docs/reports/phase-4.md
docs/reports/examples/story.jpg
docs/reports/examples/story.json
```

Também mudaram manifests/lock, config/exemplo, configuração e testes, cliente
HTTP e contrato de downloads, worker editorial e regressões de texto curto,
supervisor, admin/main, Store::editorial_commit, Docker/Compose, CI, README,
runbook e matriz. Todos os arquivos de evidência phase-4-* são saídas reais.

## Implementado

Imagens externas precisam de domínio, licença, direitos, crédito, vínculo com a
matéria/entidades, revisor, data e hash exato. A preferência é press kit, fonte
primária, og:image revisado e banco licenciado. Sem candidato seguro, o sistema
produz arte original com o título checado. Não são usadas buscas irrestritas nem
uma declaração falsa de detecção visual automática.

Downloads mantêm o cliente compartilhado e as garantias de DNS público, robots,
timeout e redirects. O teto específico de imagem é 8 MiB. JPEG/PNG/WebP são
confrontados com os bytes; dimensões e alocação são limitadas desde a leitura do
cabeçalho. O padrão permite até 16 MP e 12000 px por dimensão, com largura mínima
de 1000 px e decoder de até 96 MiB. EXIF é aplicado e removido do resultado.

O template usa JPEG 90, canvas 1080×1920 preto, Inter oficial embutida e medidas
do blueprint. Título, resumo, etiqueta e fonte preservam tamanhos, cores e
entrelinhas definidos. Até 10% de redução pode ser aplicada; além disso, pede-se
reescrita mais curta, sem cortar palavras e sem reiniciar o orçamento editorial.
Glifos ausentes e controles invisíveis/bidirecionais são bloqueados. Fontes do
sistema não participam do render.

Decode/render/hash ficam em spawn_blocking. O worker serial mantém memória
limitada, renova leases e respeita pausa. O JPEG é escrito com temporário
exclusivo, sync e rename; depois, estado RENDERED, metadados, auditoria e outbox
schedule são confirmados numa transação com fencing. Uma falha antes do commit
pode deixar arquivo órfão, sem avanço do estado ou publicação. A retomada usa
novo token e o mesmo caminho por hash.

Nova evidência também invalida RENDERED/QUEUED e cria verificação para substituir
o candidato antigo. A publicação deverá conferir novamente revisão e estado.
Compose conserva artefatos em volume, com aplicação não root e rootfs somente
leitura. A licença da Inter acompanha a imagem Docker.

## Verificado

```text
cargo fmt --check
exit_code: 0
cargo clippy --workspace --all-targets --locked -- -D warnings
Finished `dev` profile [unoptimized] target(s) in 4.45s
cargo nextest run --workspace --locked --test-threads 1 --no-fail-fast
Summary [  87.325s] 59 tests run: 59 passed, 0 skipped
cargo llvm-cov --package stories-core --package stories-normalize --package stories-cluster --package stories-verify --package stories-editorial --package stories-imaging --locked --fail-under-lines 80 --summary-only
TOTAL: 1217 linhas, 97 não cobertas, 92.03%
```

A cobertura agregada de domínio foi **92,03%**, acima do mínimo de 80%. Ela não
representa cobertura dos módulos SQL ou do supervisor da aplicação.

- Formatação e Clippy com -D warnings. Resultados integrais em evidence/phase-4-*.
- Suite completa com Postgres/pgvector reais, contratos HTTP e propriedades.
  O fixture compacto local tem os mesmos binários do oficial, conforme ADR-004;
  CI usa a imagem oficial.
- Seis referências visuais: título curto, longo, atualização, correção, acentos
  e redução de fonte. Comparação por pixels do JPEG decodificado, sem diferenças;
  margens seguras são conferidas também antes da compressão.
- Foto com transparência, proporção e cantos arredondados; remoção de metadados;
  texto excessivo, glifo ausente e alteração de direção visual bloqueados.
- MIME falso, truncamento, cabeçalho de bomba de pixels, largura insuficiente,
  idade da revisão, marca/genericidade declaradas, origem, licença e hash.
- Ordem de preferência e identidade de og:image. Download tem limite separado
  das matérias e conserva Content-Type, sem relaxar SSRF/robots/redirects.
- CHECKED até JPEG/RENDERED em dry_run, sem linha de publicação; job duplicado
  sem duplicar schedule; rollback na outbox; lease vencido após JPEG; retomada;
  invalidação de imagem já renderizada e pausa.
- Reescrita por layout conserva contador, verifica fontes novamente e pede texto
  menor. Descumprimento do limite curto esgota o orçamento e rejeita o evento.
- O comando --render-example gerou o JPEG e layout anexos sem banco ou chaves.
  O exemplo é sintético e não passou por avaliação de notícia real.

O CI verifica novamente o SHA, incluindo release, build Docker, render dentro
da imagem com rede desabilitada/rootfs somente leitura e benchmark. O resultado
remoto deve ser associado ao commit da PR; este relatório preserva a evidência
local. Não foi construído Docker localmente nesta fase, por limite de disco do
ambiente com driver VFS.

## Benchmark local em release

Rust 1.100.0, x86_64, AMD EPYC 9V74. Criterion: 20 amostras, aquecimento de 1 s,
medição de 3 s. Um segundo executável mediu 50 renders com arte e 20 com imagem
sintética de 4000×4000 já decodificada. Sem rede, Postgres ou LLM nesta medição.

| Medida | Resultado |
| --- | --- |
| Inicialização das fontes | 1,25 ms |
| Primeiro render | 62,31 ms |
| Arte original, mediana | 53,62 ms |
| Arte original, p95 | 76,46 ms |
| Imagem sintética 16 MP, p95 | 85,20 ms |
| Máximo observado na série de arte | 413,77 ms |
| RSS de pico, incluindo teste de 16 MP | 162800 KiB, cerca de 159 MiB |
| JPEG do exemplo | 131631 bytes |

Criterion estimou 54,39 ms para arte e 81,85 ms para imagem de 16 MP. Intervalos
e outliers estão no log integral. O p95 e o RSS ficaram abaixo dos gates adotados
de 150 ms e 300 MiB. O pico de 413,77 ms não foi descartado: não há prova de
limite absoluto de 150 ms por execução. A interpretação por percentil e essa
limitação estão registradas no ADR-009. A causa do pico não foi isolada.

A medição não identifica o gargalo da aplicação completa. Detecção real, rede,
LLM, armazenamento e comportamento sob carga ainda exigem os marcos seguintes.
Não se afirma que CPU é o menor gargalo sem medir esses outros estágios.

## Pendências e desvios

- Detecção automática de marca d'água, genericidade e pertinência não foi
  implementada. A alternativa é revisão humana por hash e arte original como
  padrão, conforme ADR-009. Sem manifesto aprovado, não há foto externa.
- A meta de render foi interpretada como p95; máximo absoluto não foi comprovado.
  A VPS final precisa repetir benchmark e acompanhar caudas de latência.
- Memória e detecção da aplicação inteira não foram medidas. O resultado de RSS
  é do renderizador isolado, não do serviço com todas as fontes/LLM ativos.
- Busca autenticada em bancos de imagem e avaliação visual por modelo não foram
  exercitadas. Configuração de direitos/licenças reais cabe ao operador.
- Limpeza de órfãos/retentiva, alertas e stress do serviço completo ficam na fase 6.
- Coleta real por 24 horas e fidelidade com LLM real permanecem pendentes.
- R2, publicação Meta, reconciliação e caos de publicação pertencem à fase 5.
  Nenhum Story foi publicado e nenhuma implantação de produção foi feita.

O próximo passo é implementar a fase 5 em dry_run/staging, usando a revisão e o
hash do artefato antes de upload ou publicação e preservando os gates pendentes.
