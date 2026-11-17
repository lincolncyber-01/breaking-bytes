# Runbook da fundação

Escopo atual: fundação e captura. O pipeline editorial e clientes Meta/R2 ainda não existem.

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
o banco não responde ou dry_run=false. A resposta declara scope=foundation.
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

CHECKED cria trabalho render com revisão. Ainda não há consumo desses trabalhos
ou publicação. Quando renderização for implementada, conferir estado, revisão e
pausa novamente; trabalhos de eventos SUPERSEDED não podem produzir Stories.
