# Lançamento e aceite na VPS

Os procedimentos abaixo usam serviços reais e permanecem a cargo do
proprietário. Os testes automatizados não executaram essas janelas. Comece por
[tokens](tokens.md), [backup](backups.md), [publicação](publication.md) e
[matriz de requisitos](requirements.md). Não forneça segredos em argumentos,
URLs, arquivos do repositório ou relatórios. Use seu provedor de segredos para
injetar o ambiente dos processos e reapresentá-lo após reboot.

## Preparação e release

1. Revise o PR de entrega contra main e os checks. Configure proteção de main
   com revisão independente e os checks quality, domain-coverage,
   render-performance, dependencies, operations, image-scan e fuzz. CODEOWNERS
   registra o proprietário; ele não substitui uma segunda pessoa nem configura
   proteção do branch automaticamente.
2. Crie os environments GitHub release e production, restritos a main, com
   aprovador. Cadastre os segredos descritos em `.env.example` no ambiente
   apropriado; o workflow de deploy usa os nomes sem prefixo. Use conta/bucket
   exclusivos em staging. Não registre um runner de produção que execute PRs.
3. Depois do merge e checks verdes em main, execute manualmente o workflow
   “Publicar versão verificada”, versão v0.1.0. Para outra versão, altere a
   versão do workspace e os caminhos internos, atualize Cargo.lock, revise e
   valide antes. Baixe o artefato verified-release e conserve release.json,
   SBOM e scans. O workflow publica app e backup no GHCR e não sobrescreve uma
   tag existente. Falha parcial requer investigar e emitir outra versão.
4. Prepare VPS Linux x86-64, Docker Engine/Compose atual, Python >=3.11 e
   espaço para imagens, volumes e dumps. Dimensione inicialmente com pelo
   menos 2 GB e meça a carga real. Crie usuário dedicado breaking-bytes, clone
   a revisão aprovada em `/opt/breaking-bytes`, mantenha a configuração não
   secreta em `/opt/breaking-bytes/config` e o estado privado em
   `/var/lib/breaking-bytes`, dono do usuário e modo 0700. Acesso ao grupo
   docker permite administrar o host; limite-o ao operador/runner dedicado.
5. Instale o runner self-hosted nesse host com labels linux, x64 e
   breaking-bytes-production. Configure firewall; PostgreSQL e métricas ficam
   privados. Caddy/TLS e dashboards seguem os guias de operação. Não exponha
   8080/8081 à internet.

Defina STORIES_IMAGE e BACKUP_IMAGE com os digests de release.json. Para pull
privado, forneça GITHUB_ACTOR e GITHUB_TOKEN somente leitura de packages pelo
ambiente ao helper. A CLI de implantação também aceita GHCR_READ_TOKEN. Faça o
bootstrap antes do primeiro rollout; senhas de papéis são hexadecimais distintas
de 32 a 128 caracteres. DATABASE_URL usa stories_runtime e host postgres;
MIGRATION_DATABASE_URL usa stories_owner. A conexão administrativa existe apenas
durante o provisionamento.

```sh
python3 tools/docker_registry.py pull "$STORIES_IMAGE"
docker compose -f deploy/docker-compose.yml up -d --wait postgres
docker compose -f deploy/bootstrap-compose.yml run --rm --pull never provision
```

Configure os jobs de backup/restauração e seus segredos conforme backups.md.
O lançamento exige backup verificado nas últimas 30 h e teste de restauração
nos últimos 35 dias. Valide a recuperação, o hold de segurança e um alerta real.

## Staging separado

Forneça todas as variáveis STAGING_* do catálogo e STORIES_IMAGE. Para comparar
contas, mantenha IG_USER_ID de produção e STAGING_IG_USER_ID distintos. Buckets
e credenciais R2 devem ser exclusivos. O projeto breaking-bytes-staging possui
volumes e banco próprios; o helper substitui conexões e token administrativo
antes da interpolação do Compose. O bootstrap usa exclusivamente os segredos
STAGING_BOOTSTRAP_DATABASE_URL e STAGING_*_DATABASE_PASSWORD.

```sh
python3 deploy/scripts/staging.py prepare
python3 deploy/scripts/staging.py start
```

O padrão simula. Inspecione os JPEGs e a trilha editorial; depois faça o ensaio
real na conta de teste com `python3 deploy/scripts/staging.py start --publish`.
Esse comando publica de fato e exige intenção explícita e conta distinta.
Para a API administrativa de staging, injete STAGING_ADMIN_TOKEN como ADMIN_TOKEN
no processo do operador e use `python3 deploy/scripts/control.py status --port
8081`. Faça o mesmo para pause/resume. Teste permissão, JPEG acessível pela Meta,
expiração/renovação do token, cota, reconciliação, Telegram e kill switch. Pare
com `python3 deploy/scripts/staging.py stop`; o comando preserva os volumes.

## Coleta 24 h e simulação 48 h

Injete no operador o ambiente completo de produção, incluindo chave LLM,
Telegram e R2. O helper habilita os estágios, alertas e verificação de backup.
Configure previamente fontes, origin_group, User-Agent SEC com contato real,
modelo LLM e manifesto de imagens. Mudanças nesses arquivos ou nas identidades
Meta/R2 invalidam o vínculo do aceite e exigem nova observação da configuração.

```sh
python3 deploy/scripts/deploy.py --mode dry_run --image "$STORIES_IMAGE" --cap 10 --resume
python3 deploy/scripts/acceptance.py observe collection /var/lib/breaking-bytes/acceptance/collection.json
python3 deploy/scripts/acceptance.py observe dry_run /var/lib/breaking-bytes/acceptance/dry_run.json
```

Crie antes o subdiretório acceptance com dono dedicado e modo 0700. Cada comando
permanece observando até completar a janela real; rode-o em sessão persistente
ou instale `deploy/systemd/breaking-bytes-acceptance.service`. Para selecionar a
etapa no systemd, use drop-in com
`Environment=STORIES_ACCEPTANCE_STAGE=collection`, dry_run, canary ou promoted,
e injete ADMIN_TOKEN pelo gerenciador de segredos. Instale o código no caminho
fixo da unidade. Um relatório existente não é sobrescrito: arquive a tentativa
anterior antes de iniciar outra. Interrupção conserva evidência parcial, que
não permite aprovação. Reinício/mudança de implantação durante a observação
invalida o relatório e exige nova janela completa.

O observador precisa rodar no host do daemon local, com acesso a Docker e
`/proc/PID/status`; mede VmHWM, não apenas memória ociosa. Verifica disponibilidade
>=99,5%, fluxo, fontes sem silêncio >10 min, ausência de dead letters/falhas e
pico do processo <300 MB. Isso não comprova o SLO mensal nem o orçamento de toda
a VPS. Avalie também memória dos serviços e concorrência sob carga real.

## Revisão e recibo

Revise uma amostra representativa de artigos, agrupamentos, textos e imagens,
incluindo contradições e notícias recusadas. Execute a avaliação editorial com
corpus real rotulado. Zero afirmações sem fonte, zero duplicatas semânticas e
zero falsas aceitações precisam ser constatados, não presumidos de contadores.

Grave fora do repositório um JSON de revisão com `reviewed_by` (identificação),
`reviewed_at` (UTC ISO-8601 atual), os inteiros `unsupported_claims`,
`semantic_duplicates`, `false_acceptances` (resultados da revisão) e objeto
`attestations` com estes oito booleanos:

| Campo | Evidência que deve ser revisada |
| --- | --- |
| source_terms | Robots, termos, licença, independência e contato SEC |
| semantic_deduplication | Amostra real de eventos, paráfrases e duplicatas |
| editorial_fidelity | Corpus real, checagem, números, atribuições e PT-BR |
| real_staging | Story real na conta de teste e ciclo de vida do token |
| real_alert | Alerta Telegram efetivamente recebido |
| real_backup_restore | Backup R2 e restauração real em cluster isolado |
| kill_switch | Pausa persistida, auditada e conferida no destino |
| peak_memory_load | Pico e serviços da VPS sob carga representativa |

Não há um arquivo aprovado de exemplo. Todos os atestados precisam ser true e
os três contadores precisam ser zero para o recibo ser emitido. Obtenha também
relatório SQL somente leitura, usando como início o started_at da janela que
estiver sendo aprovada. No dry-run, use o started_at de dry_run.json; nos passos
seguintes, o do canário/promoção. A SQL mede detecção das fontes primárias
elegíveis e publicação de eventos N1 reais; não inventa percentis sem amostras.

```sh
python3 deploy/scripts/sql_report.py --since "$WINDOW_START" --output /var/lib/breaking-bytes/acceptance/sql.json
python3 deploy/scripts/acceptance.py approve --report /var/lib/breaking-bytes/acceptance/collection.json --report /var/lib/breaking-bytes/acceptance/dry_run.json --review /var/lib/breaking-bytes/acceptance/review.json --sql-report /var/lib/breaking-bytes/acceptance/sql.json --output /var/lib/breaking-bytes/acceptance/approved.json
```

WINDOW_START é uma data não secreta extraída do relatório, não um valor a ser
arbitrariamente encurtado. SQL deve cobrir a janela completa, ter no máximo
30 h de idade, N1 detectado e p95 <120 s, sem intenções pendentes ou dead jobs.
A revisão deve ter no máximo 24 h ao emitir o recibo; ele vale sete dias, mas a
SQL continua sujeita à validade de 30 h. Os hashes impedem trocar os arquivos
depois de aprovar; não são uma assinatura independente do operador local.

## Canário e aumento gradual

Pelo workflow “Implantar na VPS”, escolha canary, digest exato, cap 10 e resume
somente após conferir o recibo no caminho privado. Alternativa local equivalente:

```sh
python3 deploy/scripts/deploy.py --mode canary --image "$STORIES_IMAGE" --cap 10 --receipt /var/lib/breaking-bytes/acceptance/approved.json --resume
python3 deploy/scripts/acceptance.py observe canary /var/lib/breaking-bytes/acceptance/canary.json
```

Esse passo publica de fato. Durante 72 h confira zero duplicações, perdas
pendentes e rejeições de qualidade, p95 N1 origem→publicação <300 s, cotas e
memória. Refaça SQL/revisão/recibo com nomes novos e acrescente `--report` do
canário à aprovação. Conserve os relatórios collection/dry_run vinculados à
mesma imagem/configuração. Instale o novo recibo como approved.json somente
depois de conferir os arquivos referenciados; não edite um recibo existente.

Promova com `--mode promote --cap 20`, depois observe promoted por 72 h. Refaça
as evidências e aprove mantendo apenas o relatório promoted mais recente
junto de collection/dry_run/canary. Só então aumente para 40; repita por 72 h
antes de 80. O helper não permite pular etapas e exige que o relatório anterior
tenha o teto esperado. A cota Meta real e reservas continuam limitando o envio.
Uma imagem/configuração nova exige refazer o aceite vinculado à nova versão.

## Recuperação

```sh
python3 deploy/scripts/control.py pause
python3 deploy/scripts/control.py status
python3 deploy/scripts/deploy.py --mode rollback
```

Rollback exige imagem anterior em cache, não executa migrations reversas e não
restaura banco ou volume. Fica pausado para inspeção. Se a migração anterior
introduziu quebra incompatível, não há rollback automático seguro: mantenha
workers parados e aplique correção compatível. Falha de readiness tenta retornar
à imagem anterior sob pausa; falha da própria recuperação grava
failed-deployment.json e exige intervenção. Nunca use down --volumes, apague
intenções nem reenvie uma publicação incerta. Reconcilie primeiro pelo runbook.

O workflow de deploy não inicializa contas, runner ou segredos automaticamente.
Execução manual só existe após integrar os workflows em main. A disponibilidade
mensal e a aceitação de produção ficam registradas pelo proprietário após
observar o destino real.
