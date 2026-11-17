# ADR-015 — Bloqueios e resolução administrativa

Data: 17/11/2026. Estado: implementação, com aceite real de staging pendente.

O blueprint exige pausa global, bloqueio de fontes/termos/temas, reconciliação
idempotente e tratamento conservador de respostas perdidas. Não há prova
documentada de correspondência automática entre creation_id e um ID encontrado
na lista de Stories. A associação não pode ser inferida por horário ou posição.

As regras ficam persistidas no Postgres. Alterá-las exige autenticação e pausa.
Fonte corresponde ao nome do cadastro; termo/tema corresponde a palavras ou
expressões literais, com NFC, caixa e separadores normalizados. Acentos são
preservados; aliases exigem regras adicionais. São permitidas mil regras ativas,
com paginação administrativa. Valores livres não são copiados para logs.

O bloqueio é reavaliado na captura, confirmação editorial, renderização,
agendamento e barreira anterior ao efeito remoto. Uma regra nova impede o
candidato anterior, mesmo quando seu JPEG já existe. Operação iniciada antes da
barreira não é retroativamente apagada; intenção remota já registrada continua
sendo reconciliada. A regra não elimina o histórico.

A resolução exige pausa, nenhum lease ativo, conta/modo/container esperados,
PUBLISHED consultado na Meta, ID visível na conta e atestado do operador sobre a
correspondência exata. Esse atestado é registrado como origem operator_attestation,
nunca como associação inferida automaticamente. Um UUID e hash do pedido dão
idempotência; reutilizar UUID com outro conteúdo é conflito. Sem prova suficiente,
o estado incerto permanece e nenhum novo POST é autorizado. Mídia já fora da lista
ativa pode exigir investigação manual; não há override que invente prova remota.

O tempo de criação do container é separado da reserva e limita polling de
IN_PROGRESS a cinco minutos. Registros legados usam a hora de reserva como
fallback conservador; nenhuma hora de criação remota é inventada. Simulação usa
simulated_at e mantém published_at vazio; a migration conserva o horário anterior
de simulações existentes.

Retry-After aceita segundos ou data HTTP, com limite de um dia. Antes da barreira
ele define o mínimo de retry; depois dela define somente o momento de reconciliação,
sem repetir media_publish. Códigos numéricos Graph são preservados sem mensagens
externas. Os pesos de prioridade são configuráveis, finitos e somam um; os sinais
conservadores continuam exigindo calibração por corpus real.

O teste de processo compila um binário separado, acessível somente com o recurso
test-support e Graph em loopback. O servidor simula o efeito antes de atrasar a
resposta; SIGKILL encerra o processo, outro processo reconcilia e a resolução
auditada anexa o ID. A imagem de produção não inclui esse binário. Isso testa
término de processo real contra contratos, não equivale a staging Meta real.

O terminal gerenciado ficou indisponível durante este incremento. GitHub Actions
é o ambiente de execução para formatter, compilação e testes. O workflow auxiliar
tem apenas leitura: emite alterações formatadas para revisão, sem fazer push.
Resultados efetivos do CI são registrados no PR; não são presumidos neste ADR.
