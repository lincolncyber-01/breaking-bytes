# ADR-005: fallback conservador e transações de ingestão

Data: 16/11/2026. Estado: implementado; avaliação semântica pendente.

O blueprint permite fallback por hash e entidades quando embeddings não estão
disponíveis. Esta fase implementa esse modo explicitamente, identificado como
hash_entities na API e na auditoria. Não há vetor simulado ou hash apresentado
como embedding. A coluna vector(384) e seu índice continuam disponíveis, mas os
embeddings permanecem nulos. fastembed e o modelo multilíngue ainda precisam de
download, integração e benchmark de memória antes de serem habilitados.

O hash BLAKE3 usa título sem acentos, pontuação ou diferenças de caixa. O fallback
combina entidades comuns e Jaccard dos tokens do título, no limiar configurado.
Fatos numéricos novos são candidatos a atualização; precisam de verificação na
fase 3. Eventos publicados mantêm seu estado e recebem um evento filho para
novos fatos. Esse filho começa em DETECTED e passa a CLUSTERED antes de verificar.
Artigos equivalentes sem fato novo são associados ao evento existente.

Este fallback privilegia precisão e pode deixar escapar paráfrases. Não demonstra
zero duplicatas semânticas. A publicação permanece indisponível enquanto essa
garantia não for avaliada com o conjunto editorial e o modelo escolhido. Não
confundir unicidade de URL no banco com identidade semântica de eventos.

Cada lote confirma artigos, sightings de origem, jobs de cluster e validadores
HTTP na mesma transação. Um cursor não avança antes da durabilidade dos artigos.
A comunicação em memória é limitada e tem confirmação após commit. Uma falha
de normalização rejeita o lote e conserva o cursor, sem descarte silencioso.

O cluster usa lock transacional curto para evitar dois eventos criados por uma
corrida de trabalhadores. Estado, associação, auditoria, job de verificação e
conclusão com fencing são atômicos. Cada artigo novo em CLUSTERED produz um sinal
de verificação, permitindo reavaliar N3 quando chegam confirmações. A fase 3 deve
consumir esses sinais com idempotência por estado e revisão do evento.

A fonte principal é escolhida por peso e depois detecção; sightings preservam
as origens de uma mesma URL. A verificação deve consultar essa proveniência, sem
contar republicações da mesma origem como confirmação independente.

Os candidatos são limitados a 200 por janela para controlar trabalho e memória.
O marco de 24 horas exige observação real e revisão de agrupamentos; os testes
curtos de concorrência, rollback e rajada não substituem esse marco.
