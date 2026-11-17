# Controles operacionais

Aplique a migration 0008 antes do binário novo. Faça backup antes da implantação.

A Admin API exige Bearer ADMIN_TOKEN e acesso privado. PUT /admin/pause aceita
{"paused":true}. Alterações da blocklist e resolução exigem pausa persistida;
despause somente depois de conferir estado e auditoria.

GET /admin/blocklist?after_id=0 devolve items e next_id. PUT /admin/blocklist
aceita kind (source, term ou topic), value (até 160 caracteres) e blocked.
Fonte usa o nome do cadastro. Palavras e expressões preservam acentos e têm
comparação por NFC/caixa/separadores. O corpo não aceita campos adicionais.
Uma regra removida fica inativa para conservar o histórico. As barreiras de
publicação consultam as regras atuais, mesmo para candidatos já renderizados.

POST /admin/publications/ID/resolve aceita request_id (UUID não nulo),
expected_status, expected_container_id, media_id e attest_creation_mapping=true.
O operador precisa verificar a associação entre o container e a mídia específica.
O serviço consulta PUBLISHED e a presença do ID na conta configurada, mas essas
consultas, isoladamente, não demonstram a correspondência. A associação é
atestada pelo operador e auditada com essa origem. Nenhuma resolução cria Story.

expected_status deve ser uncertain, publishing ou published_unresolved. Exige
modo real, token/preflight saudáveis, pausa e ausência de lease ativo. Um pedido
repetido com o mesmo UUID e conteúdo devolve o recibo existente; outro conteúdo
com o mesmo UUID é conflito. Não force lease de worker vivo a expirar. Aguarde-o
encerrar ou interrompa a instância antes da intervenção.

Sem mídia visível ou confirmação PUBLISHED, investigue na conta e mantenha a
incerteza. A API não inventa ID nem autoriza retransmissão cega. Após resposta
bem-sucedida, confira publications, audit_log e o recibo e então despause.

publishing.priority_weights configura cinco valores na ordem impacto, novidade,
confiabilidade, alcance e urgência. Eles devem somar um e ser finitos entre zero
e um. Alterar pesos não calibra os sinais. Valide em corpus revisado e mantenha
dry_run durante ajustes.

Simulação registra simulated_at; published_at fica reservado a confirmação real.
IN_PROGRESS usa container_created_at e no máximo cinco minutos de polling.
Retry-After conserva o atraso mínimo; depois da barreira ele apenas adia a
reconciliação. Consulte ADR-015 para limites e garantias.
