# Observabilidade

`GET /metrics` retorna Prometheus 0.0.4 na porta privada da aplicação. O host só
publica essa porta em loopback e o Caddy opcional exclui /metrics. Não exponha a
porta interna diretamente na Internet. `GET /admin/stats` usa o mesmo Bearer da
Admin API e retorna contagens, histogramas, fontes, fila e uptime sem conteúdo
jornalístico ou credenciais.

As séries do blueprint estão implementadas: detect_latency_seconds por camada;
detect_to_publish_seconds por camada e nível; articles_ingested_total por camada;
dedup_blocked_total por motivo url/semantic; verification_rejected_total por
motivo controlado; llm_latency_seconds por tarefa; factcheck_failed_total;
render_duration_seconds; ig_api_errors_total por código conhecido/other;
publications_total por transição de status; queue_depth, daily_budget_remaining,
ig_token_days_left e source_up por ID de fonte. Fonte ainda não consultada fica
up=0, com dez minutos de tolerância antes do alerta. Token sem prazo verificável
não recebe dias fictícios. Simulações não incrementam a latência de publicação
real. O orçamento real é zero quando a cota remota está desconhecida/obsoleta;
isso representa indisponibilidade para novos envios.

Métricas de negócio são transacionais e começam na migration 0009. Histogramas
runtime usam metrics/exporter com upkeep periódico e reiniciam com o processo.
A cópia SQL permite a visão histórica da Admin API. Falha ao escrever essa amostra
observacional gera log sanitizado; não descarta uma boa resposta editorial.

Painel opcional, com senha exclusiva GRAFANA_ADMIN_PASSWORD fornecida por ambiente:

```sh
docker compose -f deploy/docker-compose.yml -f deploy/monitoring/compose.yml up -d
```

Grafana escuta 127.0.0.1:3000 e Prometheus 127.0.0.1:9090. Use túnel SSH para acesso.
Os arquivos de provisionamento incluem fonte e dashboard: atraso médio, p95,
contadores de duplicatas/rejeições, erros, fila, memória e uptime. As imagens são
fixadas por versão e digest. Com o painel e backups simultâneos, prefira VPS de
2 GiB e meça a concorrência real; os 300 MiB do app não são o total do host.

## Telegram

Forneça TELEGRAM_BOT_TOKEN e TELEGRAM_CHAT_ID pelo ambiente e ative
STORIES__OBSERVABILITY__ALERTS_ENABLED=true. Chat deve ser um ID numérico;
credencial e texto de artigos nunca entram na mensagem. Alertas são independentes
do dry_run de Stories: a habilitação explícita autoriza entrega Telegram.
Nenhuma entrega real foi executada durante a implementação.

Condições: fonte silenciosa >10 min, nenhuma notícia >2 h, p95 real >5 min,
três publicações consecutivas failed/uncertain, token vencendo, orçamento esgotado,
qualquer bloqueio de afirmação não sustentada, fila morta, backup/restore atrasados.
Fonte e ausência de notícias são suprimidas durante pausa deliberada. Falhas de
credencial e backup continuam sendo observadas. Avisos recorrentes respeitam uma
hora entre entregas; bloqueio factual individual é enviado uma vez após ack.
As condições e tentativas ficam em operational_alerts, com lease de 30 segundos;
HTTP tem timeout de oito segundos e limite de resposta de 16 KiB. Retry-After é
limitado a um dia. Há oito entregas por ciclo, sem fila ilimitada em memória.
Uma resposta perdida pode repetir a mensagem, conforme ADR-016.

Confira os alertas com disparo real no bot do proprietário antes de liberar o
canário. Os testes HTTP simulados e os testes PostgreSQL não substituem esse aceite.
