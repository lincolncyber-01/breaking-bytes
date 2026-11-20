# ADR-002: contrato Meta e incerteza de publicação

Data: 16/11/2026. Estado: decisão de projeto; implementação na fase 5.

Fontes oficiais consultadas:

- https://developers.facebook.com/documentation/instagram-platform/content-publishing
- https://developers.facebook.com/documentation/instagram-platform/instagram-graph-api/reference/ig-user/content_publishing_limit
- https://developers.facebook.com/documentation/instagram-platform/instagram-graph-api/reference/ig-user/media
- https://developers.facebook.com/documentation/instagram-platform/instagram-graph-api/reference/ig-user/stories

O guia declara 100 publicações/24 h, mas a referência de cota informa 50.
Consultar `quota_usage` e `config` antes de publicar; não fixar 100 como verdade.
Stories usam containers e integram a contagem de containers publicados.
A validação final da cota exige credenciais da conta.

O fluxo confirmado é media com image_url e media_type=STORIES, consulta do
status_code do container e media_publish com creation_id. JPEG 9:16 de até 8 MB
é adequado. Stickers interativos não são suportados.

Instagram Login exige instagram_business_basic e instagram_business_content_publish.
Facebook Login exige instagram_basic, instagram_content_publish e pages_read_engagement,
com requisitos adicionais conforme a atribuição de papéis pelo Business Manager.
Escolhemos Facebook Login inicialmente porque a referência de stories documenta
esse fluxo de leitura para reconciliação. Acesso e App Review dependem da conta.

Uma restrição UNIQUE local só protege a intenção. Ela não torna uma operação
remota exatamente uma vez. A leitura de stories retorna IDs sem uma chave de
correlação local; sem prova de identidade não devemos assumir correspondência.
Persistir container e início da chamada; consultar status PUBLISHED do container
e reconciliar antes de repetir. Ambiguidade persistente fica em uncertain, com
publicação bloqueada e alerta. Não alegamos garantia simultânea de zero perda
e zero duplicatas durante uma partição sem suporte transacional da API.

O schema já distingue uncertain e dry_run. Testes de contrato e caos da fase 5
deverão demonstrar os casos resolvíveis e o bloqueio seguro dos ambíguos.

## Reconsulta em 20/11/2026

As quatro páginas oficiais acima responderam HTTP 200. O guia servido contém
simultaneamente os textos “100 API-published posts within a 24-hour moving
period” e “50 published posts within a 24-hour period”; a inconsistência
documental permanece. Não se resolve isso adotando um teto fixo mais alto.
Mantemos consulta de quota_usage/config e falha fechada quando a cota é
desconhecida. O mesmo material confirma media_type=STORIES, image_url,
creation_id, FINISHED e as permissões dos dois fluxos. Os trechos e hashes das
respostas consultadas estão em
[phase-7-meta-docs.json](../reports/evidence/phase-7-meta-docs.json).
O teste de caos já passou com SIGKILL real, Postgres real e Meta de contrato;
validação na conta e App Review continuam pendentes.
