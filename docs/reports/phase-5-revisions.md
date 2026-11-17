# Fase 5 — incremento de reconsulta e correção

Data: 17/11/2026. A fase 5 continua aberta. A ativação de correções é optativa e
permanece desabilitada nos exemplos. Não foi usada conta Meta, bucket R2 ou LLM real.

| Requisito do blueprint | Implementação | Verificação |
| --- | --- | --- |
| Reconsultar Stories das últimas 24 h | store/src/revisions.rs, app/src/revisions.rs, migration 0007 | contrato Postgres com reserva e baseline editorial |
| Acompanhar alto impacto por mais tempo | Corrections e WatchPolicy | janelas 24/72 h e limites validados; limiar configurável |
| Detectar título, matéria e nota | normalize::article_revision, Collector::revisit | nota no rodapé, scripts excluídos, HTTP 404 não vira retratação |
| Conservar data original | SourceRevision e guard SQL | fixture de publicação há vinte horas; nenhuma data é rejuvenescida |
| Mudança material confirmada | editorial/src/revisions.rs | citações literais antes/depois, segmento anterior e nota explícita |
| Checagem independente da revisão | Task::CheckRevision | envelope report/review; divergência de classificação rejeitada |
| Etiqueta CORREÇÃO | tipo explícito no evento e renderer existente | pipeline usa kind=correction; snapshots visuais existentes preservados |
| Transação e idempotência | publication_reviews, jobs e Store | repetição, lease vencido, retomada com outro dono e outbox única |
| Prova obsoleta | revisão de hash/identidade e revision_evidence_fresh | fonte muda durante processamento e bloqueia candidato anterior |
| dry_run completo | app/src/revisions/tests.rs | redação, checagem, JPEG, agendamento e simulação sem IDs externos |
| Pausa e modo | aquisição de jobs, guards e worker | modo diferente adia leitura; não promove simulação para real |
| SSRF e limites | transporte de coleta compartilhado | contratos existentes e regressões da coleta |
| Propriedades | testes normalize/verify | sinais numéricos, espaços, hash e metadados de observação |
| Operação e escolhas | ADR-014, config, Compose e runbook | ativação explícita, recuo e tratamento dos legados documentados |

## Evidência e limites

A compilação `cargo check --workspace --all-targets --offline` passou após a
integração inicial. Na primeira execução parcial de nextest, oito testes novos
passaram; uma fixture de citação precisou ser corrigida para conservar o newline
literal. Os testes Postgres locais foram impedidos por falta de espaço na criação
do contêiner Docker. Isso não é registrado como aprovação dos contratos de banco.
O `cargo fmt --check` passou e `cargo clippy --workspace --all-targets --offline
-- -D warnings` passou sem avisos em 2m16s. Saídas reais estão em
`docs/reports/evidence/phase-5-revisions-*.txt`. O CI deste incremento é acompanhado
no PR #5;
os resultados dos 81 testes anteriores não se aplicam automaticamente ao código novo.

A suíte local dos crates config, normalize, verify, editorial e collectors passou:
43 testes, zero ignorados, em 6,761 segundos. Foi repetido Clippy com --locked,
--all-targets e -D warnings após o bloqueio de reserva concorrente, sem avisos.
A cobertura do primeiro commit 24f1e64 passou no CI; a versão final tem seu próprio
workflow e seus resultados são registrados no PR.

A confirmação semântica precisa de corpus real com títulos traduzidos, notas
ambíguas, retratações e instruções maliciosas. Os testes simulados demonstram os
contratos e barreiras, não a acurácia do modelo. A extração observa os trechos
limitados das fontes e pode perder mudanças fora deles. Publicações legadas sem
snapshot não recebem baseline inventado. Janela ampliada de 72 horas e limiar
0,8 são escolhas configuráveis registradas na ADR-014.

Continuam pendentes no escopo total da fase 5: staging Meta/R2 real, avaliação
editorial real, interrupção do processo em staging, reconciliação administrativa
auditada para casos ambíguos, integração externa de rotação do segredo e calibração
de prioridade. A fase 6 e o canário de três dias da fase 7 não foram executados.
