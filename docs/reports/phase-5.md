# Fase 5 — incremento de publicação, ainda em andamento

Data: 17/11/2026. Este incremento não encerra a fase 5 nem declara produção pronta.

Foram adicionados crates scheduler e publisher, migration 0005, worker de
publicação, configuração R2 por ambiente, preflight, simulação e reconciliação.
O protocolo grava a intenção antes do efeito remoto, vincula conta/modo/revisão,
e impede repetição automática de media_publish após perda da resposta.

| Requisito | Implementação | Verificação e limite |
| --- | --- | --- |
| Pesos, cota e intervalo | crates/scheduler/src/lib.rs | testes de propriedade e fronteiras; sinais conservadores, ADR-011 |
| Cota real da conta | publisher/src/graph.rs, store/src/publication.rs | contrato quota_usage/config; conta real pendente |
| R2 temporário, SigV4 e JPEG | publisher/src/r2.rs | assinatura offline, PUT/GET/DELETE simulados e hash; R2 real pendente |
| Intenção única e outbox | migration 0005 e store/src/publication.rs | Postgres 16/pgvector, duplicidade e lease antigo |
| Container Stories e media_publish | publisher/src/graph.rs | contrato JSON, autenticação, estados e resposta limitada |
| dry_run integral após render | app/src/publication.rs | JPEG real do exemplo, status simulado, zero IDs/objeto externos |
| Reconciliação | app/src/publication.rs | Stories ativos + PUBLISHED; não associa ID por horário |
| Queda após efeito/perda de resposta | app/src/publication/tests.rs | estado durável, novo lease e contador HTTP: um POST |
| Queda antes do envio após barreira | mesmo teste | mantém uncertain, não repete nem elimina intenção |
| Retomada após container persistido | mesmo teste | reutiliza container e conclui publicação simulada na API de contrato |
| Revisão nova antes do envio | mesmo teste | reserva obsoleta falha sem efeito externo |
| Limpeza segura | store cleanup_candidates + worker | incertos excluídos; terminais aguardam TTL |
| Permissões e fluxo atual Meta | ADR-010, docs/publication.md | fontes oficiais consultadas; App Review e conta real pendentes |
| Formatação, Clippy e suíte | docs/reports/evidence/phase-5-* | saída real dos comandos locais, atualizada neste incremento |
| Imagem e release de produção | workflow CI | execução do novo commit deve ser conferida no GitHub |

A simulação consome RENDERED e QUEUED sem fingir um evento PUBLISHED. A coluna
publications.status=dry_run e a auditoria identificam o resultado; não há container,
media_id ou upload R2. O modo real não tem readiness até passar o preflight.

A exclusão por conta serializa decisões de orçamento; conexões SQL não ficam
presas durante HTTP. O lease é renovado enquanto o futuro trabalha. Antes da
barreira, erros respeitam o limite de tentativas; depois dela, só há reconciliação.
Queda ou cancelamento depois da barreira não libera uma nova publicação.

## Verificado neste incremento

Foram executados cargo fmt --check (código 0), cargo clippy --workspace
--all-targets --locked -- -D warnings (sem avisos) e cargo nextest run --workspace
--locked --test-threads 1 --no-fail-fast: 69 testes passaram, zero ignorados,
98,272 segundos de execução. Os comandos e sua saída real ficam nos arquivos de evidência. A suíte inclui os
contratos de feeds, propriedades do domínio, Postgres, editorial, hash e snapshots
visuais já existentes, além dos novos testes. O Postgres local usa a imagem
achatada breaking-bytes/pgvector-fixture:0.8.2-pg16 documentada no ADR-004. A CI usa
pgvector/pgvector:0.8.2-pg16. Os testes HTTP não acessam Meta ou Cloudflare reais.

O caos exercitado injeta perda da resposta e troca do dono do lease em fronteiras
persistidas. Não foi executado SIGKILL contra o processo conectado a uma conta
Instagram. O teste demonstra ausência de POST duplicado nos cenários modelados;
não demonstra entrega automática em toda falha possível da API externa.

## Pendências para encerrar a fase 5

1. Implementar monitoramento de validade e renovação do token por fluxo OAuth,
   confirmando os diferentes prazos e tipos de token na documentação específica.
2. Implementar reconsulta das fontes publicadas por 24 horas e criação de correções
   com trilha de evidência; integrar o tipo correction ao caminho editorial.
3. Implementar resolução administrativa auditada de mídia sem ID e ambiguidade.
4. Calibrar ou substituir os sinais conservadores com corpus revisado de notícias;
   medir latência e memória da aplicação completa com publicação habilitada.
5. Validar conta, permissões, token, leitura de Stories, URLs R2 e ciclo completo
   em staging. Repetir caos com término do processo no fluxo real.
6. Conferir cobertura, release e imagem no CI do commit publicado. Os benchmarks
   da fase 4 permanecem válidos para o renderizador isolado, sem extrapolação.

Não foram fornecidas credenciais Meta, R2 ou LLM neste ambiente. Não houve upload
real ou publicação na conta. As decisões e desvios estão nos ADRs 010 e 011.
A garantia forte de zero perda e zero duplicatas sob partição permanece inviável
sem prova adicional de idempotência remota; incerteza é preservada explicitamente.

## Preparação manual, em ordem

1. Criar/configurar app Meta, conta profissional e vínculo de Página aplicável;
   concluir permissões, papéis, App Review e autorização de publicação exigidos.
2. Obter token pelo fluxo escolhido e guardar IG_USER_ID/IG_ACCESS_TOKEN somente
   no ambiente. Validar o prazo real; ainda não há renovação automática neste código.
3. Criar bucket R2 e credencial limitada ao bucket. Configurar as três variáveis R2
   e lifecycle de um dia para health/, preservando stories/ incertos.
4. Configurar chave LLM apenas quando habilitar redação e validar seu corpus.
5. Preparar VPS com egress DNS validável, volume de artefatos, backup do Postgres
   e Admin API em loopback; medir recursos antes de habilitar fluxo real.
6. Concluir alertas e runbook operacional da fase 6 antes do aceite de produção.
