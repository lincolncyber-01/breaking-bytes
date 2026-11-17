# Fase 5 — incremento de ciclo de vida dos tokens

Data: 17/11/2026. A fase 5 continua aberta; este incremento não declara produção
pronta nem validação em conta Meta real.

O worker agora exige estado de credencial válido antes de novos envios. Facebook
Login tem inspeção por debug_token, com aplicativo, tipo, Página, permissões e
duas datas de expiração. Instagram Login tem recibo de emissão vinculado ao hash
e renovação documentada com preservação do valor. Origem e limites ficam visíveis.
Uma resposta com outro segredo ou uma resposta perdida exige rotação externa;
a aplicação não grava o valor em banco, arquivo, logs ou Admin API.

| Requisito | Onde foi implementado | Como foi verificado |
| --- | --- | --- |
| Tipo, datas, idade da prova e permissões mínimas | crates/core/src/token.rs | testes de fronteira e propriedade; expiração/acesso a dados distintos |
| Facebook debug_token | crates/publisher/src/token.rs | contrato HTTP com Bearer de inspeção, input_token e aplicativo esperado |
| Token PAGE sem prazo fixo | mesmo módulo e política do core | exige profile_id esperado e inspeção recente; zero não vira validade permanente |
| Instagram Login | mesmo módulo | recibo ligado ao hash, janela longa e permissões mínimas; origem declarada |
| Renovação após 30 dias e idade mínima de um dia | core e TokenMonitor | testes de política, contrato ig_refresh_token e relógio controlado por entrada |
| Resposta com valor conservado | publisher e app/src/token.rs | prazo autenticado salvo como metadado e preservado após reinício |
| Resposta com valor alterado | mesmos módulos | bloqueia; não instala apenas em memória e não expõe o novo valor |
| Barreira e exclusão entre réplicas | migration 0006 e store/src/token.rs | Postgres: somente uma reserva, compare-and-swap e rejeição de lease antigo |
| Interrupção durante a chamada | app/src/token/tests.rs | futuro HTTP cancelado após recebimento pelo servidor; retomada não repete |
| Readiness e status | app/src/lib.rs e app/src/token.rs | teste HTTP 200 com prova válida e 503 após expiração, sem esperar novo preflight |
| Revogação no Graph | publisher/src/graph.rs e worker | códigos 102/190 e 10/200–299 tipados, mensagens externas descartadas |
| Segredos somente por ambiente | config, Credential e armazenamento tipado | inspeção de metadados/auditoria por teste; nenhum valor antigo/novo presente |
| Logs detalhados sem queries de credencial | app/src/main.rs e filtro de targets | captura de eventos de dependências contendo segredo; saída exclui os valores |
| Hash para recibo | comando --token-fingerprint | binário real sem banco/Admin, saída somente com BLAKE3 |
| Dry-run sem credenciais externas | config e worker | testes de configuração e regressão de publicação simulada |
| Regressões das fases anteriores | workspace | feeds, Postgres, propriedades, editorial, caos de publicação e snapshots |

## Verificado

Foram executados cargo fmt --check, cargo clippy --workspace --all-targets
--locked -- -D warnings e cargo nextest run --workspace --locked --test-threads 1
--no-fail-fast. Formatação passou, Clippy não emitiu avisos e os 81 testes passaram,
zero ignorados, em 111,036 segundos de execução. Saídas reais estão em
`docs/reports/evidence/phase-5-token-*.txt`.

Os testes de Postgres usam a imagem local achatada documentada anteriormente,
breaking-bytes/pgvector-fixture:0.8.2-pg16. A CI usa pgvector/pgvector:0.8.2-pg16.
O ambiente de 8 GB precisou de limpeza do cache target/debug para recompilar;
o código e o repositório foram preservados. Não foi usado um serviço Meta/R2 real.

O CI anterior, do commit a10ab3a, foi conferido e terminou com sucesso em push e PR,
incluindo release, Docker, render offline, cobertura e benchmark. Esses resultados
não são atribuídos ao novo código. O novo commit precisa do próprio CI; o resultado
será acompanhado e registrado no PR #5.

## Não verificado e decisões

Não há credenciais Meta de staging neste ambiente. Permissões, tipos de token,
expiração, renovação e publicação precisam ser exercitados com a conta real.
O contrato simulado não comprova comportamento de produção. O teste de interrupção
cancela o futuro de HTTP sobre Postgres/servidor simulado; não é SIGKILL ligado à Meta.

Não conseguimos confirmar introspecção oficial acessível para Instagram Login.
O recibo inicial é fornecido pelo ambiente e não prova concessão remota de scopes;
isso é declarado no status. A leitura de cota/Stories e o teste real de publicação
continuam obrigatórios. O servidor de renovação só fornece novo prazo ao responder.

Renovação automática com valor alterado depende de mecanismo externo de rotação
do ambiente. A alternativa implementada conserva segredos somente por ambiente,
preserva o bloqueio após reinício e deixa a mudança de valor pendente para a implantação.
Na prática, o operador precisa obter uma credencial pelo fluxo Meta e atualizar a
implantação nesses casos. O daemon não guarda ou disponibiliza a resposta secreta.
Decisões completas: ADRs 012 e 013; operação: docs/tokens.md.

## Próximos passos da fase 5

1. Reconsulta, tipo correction e trilha de evidência implementados no incremento
   [phase-5-revisions.md](phase-5-revisions.md); avaliação semântica real pendente.
2. Resolução administrativa auditada de publicação incerta/sem identificador.
3. Pesos configuráveis e calibração da prioridade, sem alegar classificador pronto.
4. Integração externa de rotação de segredo e validação dos fluxos com conta real.
5. Chaos de processo/publicação em staging, medição da aplicação completa e portões
   restantes antes de encerrar a fase 5. Alertas externos e endurecimento são fase 6.
