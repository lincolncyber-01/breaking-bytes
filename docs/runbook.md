# Runbook operacional

Escopo atual: fundação, captura, editorial, renderização e publicação em dry_run;
clientes Meta/R2 e correções disponíveis para validação em staging. A fase 5 segue aberta.

## Inicialização e migrations

Confirme Rust 1.100.0, Postgres 16 e pgvector 0.8.2. Defina DATABASE_URL e
ADMIN_TOKEN por ambiente. ADMIN_TOKEN deve ter pelo menos 32 bytes aleatórios.
Execute o binário com --migrate antes de iniciar o serviço HTTP. As migrations
são versionadas, transacionais e verificadas por checksum pelo SQLx.
Não edite migrations já aplicadas; novas mudanças usam expansão compatível.

Em Compose, o serviço migrate deve terminar com sucesso. Se falhar, app não
deve iniciar. O banco conserva os dados no volume postgres-data. Não remova o
volume para tentar corrigir uma migration.

## Health checks e pausa

GET /health/live retorna 200 enquanto a API responde.
GET /health/ready executa uma consulta limitada a dois segundos; retorna 503 se
o banco não responde. No modo real, exige publicação habilitada, preflight e token
válido. A resposta declara o estágio habilitado em scope.
O comando --healthcheck sonda a readiness por TCP local, com limites de tempo.

Para pausar, envie PUT /admin/pause com {"paused":true} e token Bearer no
header Authorization. A alteração e sua auditoria são transacionais. Confira
GET /admin/status. Reiniciar a aplicação não desfaz a pausa.

O processo trata SIGTERM e SIGINT e fecha o pool após encerrar o servidor.
O serviço só escuta loopback por padrão. Em Compose, o host também publica a
porta somente em 127.0.0.1; acesso remoto exige proxy privado ou TLS.

## Falhas de banco e fila

Erros expõem apenas código SQLSTATE; DSN e mensagens externas não são registrados.
Confira conectividade, permissões e extensão vector pelo administrador do banco.
Jobs com lease vencido voltam a poder ser adquiridos; o token da tentativa antiga
fica inválido. Após o limite de tentativas, o job recebe dead_at e sai da fila ativa.
Não reaproveite o token antigo nem confirme o job manualmente.

Para investigar, consulte jobs.id, kind, attempts, lease_until, dead_at e
last_error_code. Não coloque conteúdo de artigos ou segredos em last_error_code.
Inspecione audit_log e o estado do evento antes de criar um novo job.

## Coleta e observação de 24 horas

Valide o egress antes de ativar `STORIES__COLLECTION__ENABLED=true`. O binário
recusa proxies de ambiente; não contorne a rede gerenciada da Cloud. Na VPS,
confirme robots.txt e termos de cada fonte. Para EDGAR, configure User-Agent com
contato real e mantenha o controle de frequência. Aplique a migration 0002.

Confirme no status administrativo collection_enabled e clustering_mode. Acompanhe
last_success_at, failures e next_poll_at em sources e os logs JSON de cada lote.
O validador HTTP só muda após commit dos artigos e jobs. Uma falha no lote conserva
o cursor e a próxima tentativa usa o estado durável anterior.

Inspecione artigos, article_sightings, event_articles e audit_log. Jobs verify
são consumidos pelo worker editorial somente quando llm.enabled=true.
Para o aceite de 24 horas, retenha logs completos, datas inicial/final, contagens
de artigos e eventos e amostras revisadas de agrupamento. A restrição UNIQUE de
URL não comprova ausência de duplicatas semânticas. Nenhum Story será publicado.

## GitHub e limites da validação

Use uma conexão GitHub com escrita de conteúdo e workflows no repositório.
Quando houver várias contas conectadas, selecione explicitamente a conta do
proprietário lincolncyber-01. Envie mudanças por branch e PR, preservando revisão
antes de integrar à principal. A fundação está integrada à main. A captura está na branch codex/phase-2-collection.

Proteção de branch, CI remoto, credenciais externas, backups, alertas, publicação
de teste, caos, snapshots, benchmarks e canário ainda precisam dos marcos das
fases seguintes. Esta fundação não deve ser anunciada como pronta para produção.


## Verificação e redação

Aplique a migration 0003 antes de atualizar a aplicação. Defina LLM_API_KEY
somente no ambiente e habilite STORIES__LLM__ENABLED=true. Mantenha dry_run=true.
O estágio precisa ler fontes pela rota de rede validada; não desative proxies da
Cloud para forçar a execução. Sem chave o serviço falha na configuração.

Cadastre origin_group para cada fonte B/D usada em N2. Use o mesmo valor para
sites que republicam a mesma agência ou compartilham origem editorial. A
independência editorial não é inferida de nomes de domínio. Fontes removidas ou
desabilitadas no arquivo deixam de participar após a sincronização no startup.

O status administrativo expõe editorial_enabled. Inspecione events.state, level,
reason, editorial_attempts, content_revision e verified_revision. As evidências
confirmadas ficam em assessment_json; draft_json e factcheck_json conservam as
saídas aprovadas. Esses campos contêm conteúdo editorial, portanto controle o
acesso ao banco e aos backups. Os logs não carregam matérias, prompts ou corpos
HTTP. Auditoria registra códigos, IDs e revisões.

N3 retorna em 60 segundos e expira em max_age_hours. Falhas de fonte podem
manter uma notícia válida em observação: confira robots, canonical e o metadado
article:published_time. Não substitua a data por updated/lastmod para liberar um
item. GitHub/SEC/Statuspage exigem nova confirmação pela API de origem.

429 e falhas transitórias preservam o estado e reagendam o job. Não reinicie as
reescritas por tentativa HTTP. Após duas reescritas, recusa do modelo ou falha
permanente, confira a razão persistida antes de investigar. Jobs esgotados por
quedas repetidas ficam em dead letter e exigem inspeção, como os demais jobs.
Nunca reutilize token de lease ou promova o estado manualmente.

Use --evaluate-editorial conforme o README para gravar a concordância sobre
candidatos rotulados. Erro do provedor conta como erro de avaliação, separado de
rejeição factual. O relatório não inclui chave, corpo de resposta ou texto das
matérias. Os exemplos versionados são sintéticos; o aceite exige corpus real,
revisão humana e inspeção das redações produzidas pelo pipeline. Meta inicial:
zero falsas aceitações e 100% de concordância no corpus aprovado pelo operador.
O blueprint não especifica um percentual; essa suposição está no ADR-007.

CHECKED cria trabalho render com revisão. O worker da fase 4 consome esses jobs,
gera o JPEG e confirma RENDERED mais outbox schedule. O incremento da fase 5 consome schedule em dry_run e implementa o protocolo
real com reconciliação; o aceite de produção segue pendente. Estado, revisão, versão, lease e pausa são conferidos na confirmação.


## Renderização e artefatos

Aplique a migration 0004 antes de atualizar a aplicação. Consulte docs/images.md
para revisão das imagens e operação do manifesto. O padrão produz arte original
sem acesso de rede; com fontes/LLM habilitados, a rota jornalística continua
exigindo DNS verificável e recusa proxy de ambiente.

Confira imaging_enabled no status. O manifesto é relativo ao diretório do TOML,
e artifacts_dir é relativo ao diretório de execução ou absoluto. A aplicação
precisa de escrita nesse diretório; o restante do container pode ser somente
leitura. Compose conserva os arquivos no volume story-artifacts. Não remova esse
volume ao atualizar a imagem. A distribuição mantém a licença OFL da Inter.

Os JPEGs usam event_id/revision/hash.jpg. rendered_stories contém hash, dimensões,
layout, proveniência e dry_run. Estado RENDERED e outbox schedule só são gravados
após sync do arquivo e diretório. Uma queda antes do commit pode deixar arquivo
órfão; ela não autoriza uma publicação. A retomada usa novo lease e o mesmo hash.
Não apague arquivos referenciados no banco. Limpeza automática por retenção ainda
será implementada na fase 6.

O texto que não couber após redução de 10% ganha substituto para nova verificação
e redação mais curta. O contador é preservado; no esgotamento, a razão é
layout_exhausted. Não reduza mais a fonte nem corte texto para liberar o job.
Um glifo ausente na Inter também exige reescrita ou rejeição; não usamos fontes
de fallback instaladas no sistema.

Nova evidência em RENDERED/QUEUED enfileira verificação e torna o candidato antigo
obsoleto. O consumidor schedule/publicação confere estado e
revisão antes de avançar. Não consuma essa outbox com scripts genéricos.

Os benchmarks da fase 4 medem o renderizador isolado. O p95 passou no ambiente
medido, mas houve máximo de 413,8 ms; valide a VPS final e monitore percentis.
Ainda faltam memória da aplicação inteira, detecção real, R2, Meta e caos de
publicação. Nenhum resultado desta fase comprova publicação real.

## Publicação em andamento

Leia docs/publication.md antes de alterar o modo. Aplique a migration 0005.
As intenções uncertain e published_unresolved exigem acompanhamento; não apague
registros nem reenvie por script. Preflight não substitui App Review, staging ou
validação do ciclo de vida do token. Portões estão em docs/reports/phase-5.md.

## Credenciais Meta

Aplique também a migration 0006. Leia docs/tokens.md antes de habilitar modo real.
A readiness verifica vencimento e idade dos metadados, além do preflight. O status
administrativo identifica tipo, origem e falha sem revelar o segredo. Em
rotation_required ou refresh_uncertain, obtenha uma credencial pelo fluxo Meta,
atualize o ambiente e recrie o processo; não remova a barreira por SQL. Alertas
locais são deduplicados; entrega externa ainda depende da fase 6.

## Reconsulta e correção automática

Aplique a migration 0007 antes de iniciar o binário atualizado. Ative
`STORIES__CORRECTIONS__ENABLED=true` em uma rota com DNS/SSRF validado. Mantenha
`STORIES__INSTAGRAM__DRY_RUN=true` durante a validação. Para decidir e redigir,
ative também `STORIES__LLM__ENABLED=true` e forneça a chave pelo ambiente. Sem LLM,
a reconsulta grava observações e deixa os jobs `review_revision` na fila.

O intervalo padrão é 300 segundos por publicação. A janela é 24 horas; score
maior ou igual a 0,8 estende para 72 horas, configuráveis em `[corrections]`.
O score atual é conservador e não representa impacto calibrado em corpus real.
Falha de todas as fontes aumenta o intervalo exponencialmente até uma hora,
com jitter. Nenhuma falha, remoção HTTP ou mudança apenas de data confirma retratação.
A pausa administrativa suspende novas aquisições de reconsulta e revisão.

Consulte `publication_watches`, `publication_source_snapshots`,
`publication_reviews` e `audit_log`. Os estados `ignored` e `uncertain` não criam
Story; `superseded` indica prova substituída ou expirada. O baseline é a cópia
editorial congelada na reserva original, preservada mesmo quando a tabela de
artigos recebe novas capturas. Publicações anteriores à migration, sem essa cópia,
não ganham baseline inferido. Investigue-as manualmente.

Uma decisão publicável cria evento filho `update` ou `correction`, passando de
novo por verificação, redação, checagem independente, renderização e orçamento.
A correção usa a reserva de capacidade, mas continua respeitando o teto total e
a cota remota. A etiqueta CORREÇÃO é produzida pelo template existente. A fonte
precisa conservar identidade e conteúdo confirmado até as barreiras de envio.
O acompanhamento anterior encerra após a nova publicação terminal; o novo Story
recebe seu próprio baseline. Simulação termina como `dry_run`, sem ID de mídia.

Um job com `dead_at` exige investigação da causa e retomada administrativa sob
pausa; não apague o histórico nem crie outra publicação para contornar um estado
incerto. Ao mudar entre dry_run e real, o acompanhamento antigo fica adiado e
nenhum candidato de simulação é promovido. Valide a classificação semântica em
corpus real antes de habilitar publicação real: citações literais não provam,
por si só, a relação entre uma nota e o Story anterior.
