# Complemento da fase 7 — Rollout com containers reais

Data: 20/11/2026. O ensaio amplia a validação dos contratos de comandos para
Docker Compose, processo da aplicação e PostgreSQL reais. Não equivale à
implantação na VPS nem ao aceite de serviços comerciais.

## Plano e arquivos

```text
crates/app/src/collection.rs
deploy/integration/rollout.py
.github/workflows/ci.yml
docs/adr/020-shutdown-da-coleta.md
docs/reports/phase-7-rollout.md
```

O CI executa o ensaio depois de construir a imagem release, conserva o render
offline e publica o relatório compose-rollout. A revisão da imagem é rotulada
com o SHA da execução. A identificação do projeto de teste é aleatória; a
limpeza remove exclusivamente seus containers, rede e volumes.

## Cenários

1. Bootstrap real de owner/runtime/backup e migrations como owner.
2. Readiness, modo dry-run, teto 10, retomada explícita, pausa, healthcheck
   executável e hardening do container com UID 65532 e raiz somente leitura.
3. Troca para imagem com ID distinto e rollback para a anterior, mantendo a
   pausa e uma linha sentinela de auditoria no mesmo banco.
4. Conexão de migração deliberadamente inválida, com recuperação da versão
   anterior sem restaurar snapshot do banco.
5. Token administrativo divergente na nova versão, recusa do readiness
   autenticado e recuperação da imagem anterior pausada.
6. Rede interna sem saída externa, pausas auditadas, ausência de publicações
   reais e códigos de saída zero durante as paradas normais.

O teste remapeia referências do GHCR para duas imagens locais já construídas;
não testa autenticação/pull do registry. O processo, migrations, banco,
containers e respostas HTTP são reais. A API é consultada pela própria rede
interna usando Bash/timeout da imagem PostgreSQL; printf é builtin e recebe
token por ambiente, sem segredo em argumentos de processo. Não se publica uma
porta de host na rede interna do fixture. A publicação de portas de produção
continua sujeita ao ensaio no destino real.
O teste também recusa uma senha de runtime incorreta, registra a versão do
Compose e conserva stderr sanitizado quando um comando falha.

## Correção encontrada e limites

A primeira execução completa revelou cancelamento indevido de coletores no
shutdown, registrado no ADR-020. A correção permite que eles observem o sinal
compartilhado e encerrem após a drenagem do canal. O ensaio passou a conferir
também os códigos de saída, além de readiness e dados preservados.

O ambiente local tem Docker rootless, sem controlador de cgroups, e disco de
8 GB. Para ele, foi empacotado o binário debug já compilado com suas bibliotecas
e certificados; isso é fixture de execução, não a imagem release distribuída.
O CI constrói e usa o Dockerfile original. Não se atribui ao fixture local prova
de limites de CPU/memória de produção ou de segurança da imagem release.
Nenhuma credencial real é herdada pelo teste; ele usa ambiente mínimo com
valores exclusivos e descartáveis.

O binário corrigido, rotulado 20a0552, passou localmente os seis cenários com
SCRAM e seis códigos de saída zero em 46,536 s. O reforço com senha inválida e
stdin fechado passou em 48,283 s; com Compose 5.8.0, em 45,906 s. Esses ensaios
usaram o fixture debug, não são resultados da imagem release.
Clippy local passou sem avisos em 4 min 07 s. A repetição de nextest local não
concluiu a linkedição por falta de disco; os 111 testes Rust passaram no CI.
O CI apontou depois a opção --no-tty incompatível com seu Compose; o ADR-021
registra o diagnóstico e a troca por -T. Os resultados finais da revisão
corrigida ficam vinculados ao SHA nos checks e no artefato compose-rollout.

Reprodução no host de testes, após construir e rotular a imagem:

```sh
docker build --label "org.opencontainers.image.revision=$(git rev-parse HEAD)" -f deploy/Dockerfile -t breaking-bytes:ci .
python3 deploy/integration/rollout.py --image breaking-bytes:ci --report artifacts/rollout.json
```

Não execute em um host sem recursos para os containers descartáveis. O teste
não emite recibo de lançamento e não encurta as observações de 24/48/72 horas.
Meta, R2, LLM, Telegram e GHCR autenticado permanecem no aceite externo.
