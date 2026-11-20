# ADR-020: preservar a drenagem dos coletores no shutdown

Data: 20/11/2026. Estado: correção implementada; validação no ensaio Compose.

O ensaio com o processo completo, coleta habilitada e SIGTERM mostrou uma
condição de corrida: se o worker de cluster terminasse primeiro, o supervisor
chamava abort_all nos produtores. Ao drenar o JoinSet, o cancelamento gerado
pelo próprio supervisor virava erro “coletor encerrou com panic”, encerrando o
binário com falha durante uma parada normal.

Os produtores já observam o mesmo sinal de parada. A correção conserva essa
saída cooperativa e o fechamento/drenagem do canal, permitindo confirmar os
lotes já entregues. Em falha real do worker, o retorno por erro continua
descartando o JoinSet e cancelando tarefas remanescentes. O deadline externo de
30 segundos permanece em vigor.

O teste de regressão executa rollout e rollback com containers reais, coleta
habilitada, papéis distintos e banco real. Registra os códigos de saída após
seis paradas e exige zero em todas, além de preservar uma linha sentinela de
auditoria e impedir publicações reais. HTTP administrativo é consultado de
dentro da rede interna do fixture; credenciais são transmitidas pelo ambiente,
sem token em argv. Não há alteração de fluxo de publicação, cota ou recibos de
aceite.
