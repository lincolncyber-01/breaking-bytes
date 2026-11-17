# ADR-010: publicação durável e fronteira de incerteza

Data: 17/11/2026. Estado: implementado neste incremento; aceite real pendente.
Complementa o ADR-002.

A consulta oficial atual retorna uso e configuração da cota. O guia cita 100
publicações móveis em 24 horas, enquanto a referência informa 50. O cliente pede
`fields=quota_usage,config`, exige duração de 86400 segundos e usa o menor limite
entre a conta e a configuração local. Resposta incompleta bloqueia novos envios.

A intenção é única por evento, tipo e sequência. Ela também vincula conta,
revisão, hash e dry_run. O arquivo é relido com limite e seu hash é conferido em
spawn_blocking. Render simulado não pode virar publicação real por mudança de
configuração. A simulação termina em publications.status=dry_run, sem IDs fictícios;
o evento permanece QUEUED e a auditoria informa dry_run_completed.

As transações usam lease com fencing e exclusão por conta. Estado, revisão,
verificação, idade das fontes e pausa são conferidos antes da barreira de envio.
Nenhuma transação permanece aberta durante HTTP. A barreira muda o job para
reconcile antes de media_publish. Uma retomada depois dela nunca repete o POST.

A referência do container fornece status_code, inclusive PUBLISHED, mas não uma
aresta ou campo documentado que recupere o ID da mídia publicada. Stories ativos
não têm chave de correlação com a intenção. Portanto a reconciliação não associa
IDs por horário, proximidade ou contagem. PUBLISHED permite confirmar o efeito em
published_unresolved, com media_id nulo; a ausência do ID fica explícita.

FINISHED, erro de transporte e ausência na listagem não provam ausência do efeito.
A intenção permanece uncertain, preservando objeto e reserva de cota. Há uma
limitação inevitável: uma queda entre a barreira e a chamada pode deixar uma
intenção sem efeito, bloqueada para investigação. Não há garantia demonstrada de
entrega automática sem perdas e sem duplicatas para toda partição da Meta. Esse
é um desvio explícito da garantia forte solicitada, e não uma conclusão do teste.

Novos containers podem ser recriados após perda da resposta da criação: containers
órfãos não são publicados. O único container persistido é usado para o POST final.
Limpeza só alcança estados terminais e respeita TTL após confirmação. Incertos não
são apagados. Pausa bloqueia novos envios, mas permite reconciliação.

Contratos HTTP e um teste com Postgres simulam perda da resposta, expiração de
lease, revisão nova, retomada com container persistido e queda antes da chamada.
Isso não substitui SIGKILL no processo conectado à conta de staging.

Fontes oficiais consultadas em 17/11/2026:

- https://developers.facebook.com/documentation/instagram-platform/content-publishing
- https://developers.facebook.com/documentation/instagram-platform/instagram-graph-api/reference/ig-user/content_publishing_limit
- https://developers.facebook.com/documentation/instagram-platform/instagram-graph-api/reference/ig-container
- https://developers.facebook.com/documentation/instagram-platform/instagram-graph-api/reference/ig-user/stories
