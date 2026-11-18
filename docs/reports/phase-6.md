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

As evidências abaixo distinguem resultados locais, CI e aceites externos.

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

## Evidência adicional em release e operações

A execução CI 44203846339 concluiu os três jobs com sucesso no commit 5a014bf:
qualidade (110 testes em 33,257 s, formatação, Clippy, build release e render
na imagem somente leitura/sem rede), cobertura de domínio >=80% e benchmarks.
Fuzz 44203846442 passou com três alvos, 20 segundos por alvo. O scan da aplicação
nesse commit passou sem HIGH/CRITICAL e sem segredos. Cargo-deny passou no
commit 6a93d37 (job 134477142406). As mudanças posteriores de empacotamento e
retenção de artefatos ainda dependem do resultado da execução final.

O novo benchmark release mediu captura p95 4,642 ms/lote de 200 itens, cluster
p95 2,427 ms e render p95 58,266 ms (arte original), 86,409 ms (16 MP). Pico RSS
do processo de render: 164.892 KiB. Ver [benchmarks.md](benchmarks.md).
Os arquivos de dois testes saíam no diretório do crate; o workflow usa agora
caminhos absolutos. A medição original foi preservada do stdout real, sem inventar
um artefato ausente. Cobertura é gate de linhas >=80% do domínio declarado,
não cobertura de todo o workspace.

Reexecução nativa do contrato de backup: nove testes, nenhum ignorado, 14,205 s,
com a fixture Postgres real. Uma tentativa anterior sem carregar o ambiente da
fixture falhou ao tentar obter a imagem padrão no sandbox; não foi contabilizada
como teste funcional aprovado. A execução com a imagem instalada passou.

ADR-018 registra a vulnerabilidade do backend SQLite opcional fora do binário e
a auditoria do executável entregue. O backup mínimo preserva metadados das
bibliotecas; seu build exige resolver todos os vínculos dinâmicos, sem ignorar
uma biblioteca ausente nem esconder vulnerabilidades do scanner.

## Aceites que não foram executados

Não houve credenciais para disparo Telegram real, escrita/leitura R2 real,
publicação staging ou avaliação editorial cobrada. Não houve observação real por
24 h, dry-run por 48 h ou canário por 72 h. Backup e restauração foram testados
com banco real e R2 simulado; não comprovam o volume, cron ou desastre da VPS.
A aplicação completa sob carga e o SLO mensal exigem observação no destino.


## Suíte final integrada

O teste integrado adicional parte de RSS de contrato, normaliza, ingere, agrupa,
redige/checa com cliente de contrato, renderiza o JPEG real, agenda e consome a
simulação. Uma repetição da entrada conserva uma intenção e nenhum ID remoto.
Passou em 5,076 s. A recompilação limpa terminou em 6m19s e a suíte inteira:

```text
Summary [ 143.083s] 111 tests run: 111 passed, 0 skipped
```

Clippy de todos os alvos/recursos: 3m21s sem avisos; rechecagem após formatação:
1,17 s, sem avisos. O fmt final retornou 0. Evidências `phase-6-final-*` conservam
as saídas reais. A limpeza deste ciclo atingiu apenas artefatos Cargo gerados no
workspace; nenhum diretório interno Docker cuja remoção fora recusada foi tocado.

No CI 44210767728, o LCOV registrou 2.409/2.619 linhas de domínio: 91,9817%.
A memória ociosa do binário debug no runner foi 42.744 KiB; a medição nativa
anterior de 38.808 KiB continua histórica, sem alegação de pico sob carga.
O job operacional 134488340255 passou com clientes da imagem entregue,
restauração em dois clusters reais, SSL/fuso, AWS CLI, Compose/Caddy/Prometheus,
calendários e Grafana com 12 painéis. O R2 continuou simulado.

O último bloqueio foi o inventário sobreaproximado do cargo-auditable estável;
ADR-018 registra a reprodução e o exportador ancorado em compiler-artifact real.
Seis testes Python do inventário/política passaram; o contrato do exportador
compila dois cenários Cargo reais e conserva o backend proibido quando ativado.
Também extrai a seção adicionada com objcopy, confirma que .text é idêntica e
executa o binário com o inventário embutido. O novo build/audit/scan do executável
completo ainda depende da execução do CI deste incremento.


## Auditoria do executável aprovada; minimização do backup

No commit a15e0a5, job 134497226450, o build real exportou e validou 389 crates;
cargo-audit bin passou e Trivy da aplicação terminou com zero HIGH/CRITICAL e
zero segredos. O inventário completo está versionado. O bloqueio restante foi
o runtime do backup: pip vendorizado e bibliotecas opcionais interativas/UUID.
O resumo de achados antes da minimização está nas evidências; ADR-018 explica
por que removemos também o código e as bibliotecas, sem ignorar CVEs.

O contrato adicional executou AWS CLI 2.39.6 real, sem readline/_uuid/bibliotecas
privadas, contra HTTP local e confirmou SigV4, PUT, GET, listagem e DELETE.
Backup/restauração nativos passaram novamente: nove testes em 12,991 s, nenhum
ignorado. O novo Docker build repete o contrato dentro da imagem com rede externa
indisponível. Auto prompt é explicitamente desligado, independente do ambiente.
Actionlint 1.7.12 validou os workflows; ShellCheck/Pyflakes não foram executados
por esse comando. A versão e o checksum da publicação oficial estão fixos no CI.
A aprovação final do scan dessa imagem minimizada permanece obrigatória.
