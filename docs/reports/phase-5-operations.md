# Fase 5 — controles operacionais e término real do processo

Plano do incremento: acrescentar política de bloqueio auditada, reconciliação
administrativa com prova e atestado do operador, pesos de prioridade configuráveis,
Retry-After e interrupção do processo depois do efeito externo simulado.

Arquivos: `migrations/0008_operations.sql`, `crates/core/src/policy.rs`,
`crates/store/src/operations.rs`, `crates/app/src/admin.rs`, seus testes,
`crates/app/src/bin/publication_chaos.rs`, `crates/app/tests/process_chaos.rs`,
integração nos estágios existentes e ADR-015.

A pausa é obrigatória para alterar regras e resolver uma publicação. O recibo UUID
é idempotente e o payload diferente com o mesmo UUID é rejeitado. A API verifica
conta, estado remoto do contêiner e presença do ID nos Stories ativos. Esses dados
não estabelecem sozinhos a associação entre contêiner e mídia: o operador precisa
atestá-la, e a origem desse atestado fica na auditoria.

A blocklist cobre fonte, termo e tópico, inclusive nas barreiras imediatamente
anteriores ao envio. A reconciliação de intenções existentes continua durante a
pausa. Simulações usam `simulated_at`, sem datas fictícias de publicação real.

Primeira suíte local: 104 testes em 129,107 s; 101 passaram e três falharam.
As duas falhas de TLS pertenciam à inicialização isolada dos testes. O teste de
SIGKILL não observou o efeito remoto; o diagnóstico passou a preservar stderr do
processo de contrato. As fixtures também foram corrigidas para atravessar todas
as transições exigidas pelo banco. Esses achados não são apresentados como teste
aprovado. A execução final é registrada abaixo após a correção.

Clippy, compilação de todos os alvos e formatação passaram no CI do commit
8d9d805. Conta Meta, bucket R2 e LLM reais não foram utilizados.

## Resultado local final

Após corrigir o auxiliar para validar o token pelo contrato local, sem executar
sonda R2 sobre o contêiner já pronto da fixture, a suíte completa passou:

```text
Summary [ 134.051s] 104 tests run: 104 passed, 0 skipped
```

O teste de SIGKILL passou em 5,126 s: o primeiro processo gravou a barreira e
causou o efeito remoto simulado; recebeu SIGKILL antes da resposta; o segundo
processo reconciliou sem outro POST. A associação ambígua foi conservada em
`published_unresolved` até o atestado administrativo. O teste verificou um único
POST, um único efeito e a persistência do ID confirmado. Postgres e processos são
reais; Graph é Wiremock. Não se trata de publicação em conta Instagram.
A saída completa está em `evidence/phase-5-operations-nextest.txt`.

A criação do contêiner local exigiu usar a fixture pgvector achatada documentada
no ADR-004. Bibliotecas geradas do Cargo foram comprimidas reversivelmente para
liberar espaço, conservando binários e metadados dos testes. A limpeza das áreas
internas Docker recusada pela revisão automática não foi repetida.

O aceite externo da fase 5 continua bloqueado por configuração manual: Meta/R2,
corpus editorial real, rotação externa de credencial e calibração de prioridade.
Os artefatos das fases seguintes serão construídos sem atribuir aprovação a esses
portões. A separação é necessária para entregar o repositório sem simular um
aceite de produção que não ocorreu.

`cargo fmt --check` terminou com código 0, sem diferenças.
`cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings`
terminou com código 0 em 2m38s, sem avisos. Saídas reais versionadas em
`evidence/phase-5-operations-fmt.txt` e `evidence/phase-5-operations-clippy.txt`.
Os portões automáticos locais deste incremento estão aprovados.
