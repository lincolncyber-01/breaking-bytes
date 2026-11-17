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
aguardam a fase 3 e não devem ser consumidos por um trabalhador genérico.
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
