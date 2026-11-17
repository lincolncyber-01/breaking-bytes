# ADR-013: monitoramento e renovação sem persistir a credencial

Data: 17/11/2026. Estado: implementado neste incremento; Meta real não validada.
Complementa o ADR-012 e mantém os portões externos da fase 5 abertos.

## Contratos confirmados e limite de introspecção

Facebook Login usa GET /v27.0/debug_token com input_token e autenticação de um
app access token ou token de desenvolvedor autorizado do mesmo app. Conferimos
is_valid, aplicativo, tipo USER/PAGE, permissões, expiração e expiração de acesso
a dados. PAGE também precisa corresponder a FB_PAGE_ID. Outros tipos são
recusados. Um prazo zero só representa ausência de expiração fixa para PAGE;
a inspeção periódica continua obrigatória.

Instagram Login usa o endpoint documentado graph.instagram.com/refresh_access_token,
com ig_refresh_token. A renovação exige token longo válido e pelo menos 24 horas
de idade. O padrão tenta renovar após 30 dias desde a emissão/última renovação.
Não implementamos troca periódica Facebook User como se fosse renovação Instagram,
nem estendemos artificialmente a validade de Page tokens.

Não conseguimos confirmar, em documentação oficial acessível, um contrato de
introspecção equivalente para Instagram Login. Não inventamos essa chamada.
Esse fluxo exige um recibo de emissão fornecido pelo ambiente, vinculado ao hash
do segredo: datas e permissões declaradas na emissão. A origem oauth_environment
fica visível. Isso não comprova remotamente a concessão das permissões. As leituras
reais de cota e Stories continuam no preflight; a permissão efetiva de publicação
precisa ser validada em staging. Uma resposta autenticada de renovação fornece
novo prazo e muda a origem para instagram_refresh.

## Segredo por ambiente e resposta de renovação

O usuário exige segredos somente por ambiente. Portanto o daemon não grava tokens
em arquivo, banco, auditoria, logs ou resposta administrativa. O banco recebe
somente hash, datas, categoria, identificadores e permissões de uma lista fechada.
Não guardamos o recibo bruto ou o corpo da API.

Se a renovação conservar o mesmo valor do segredo, o prazo recebido é persistido
como metadado daquele hash, inclusive para retomada após reinício com o ambiente
original. Se a Meta devolver outro valor, ele não é instalado apenas em memória:
isso perderia a nova credencial no reinício e descolaria o processo do ambiente.
O valor é descartado e o estado rotation_required bloqueia novos envios. O operador
deve obter uma credencial pelo fluxo Meta e atualizar o ambiente de implantação.

Esse é um limite explícito à renovação automática geral solicitada no blueprint.
A automação completa de um valor alterado exige um mecanismo externo de rotação
do segredo de implantação; não há tal serviço configurado neste ambiente. A
alternativa implementada renova automaticamente apenas o valor conservado e
bloqueia os demais casos. Tokens Facebook são monitorados e exigem nova emissão/
reautenticação pelo fluxo apropriado quando necessário.

## Concorrência, falha e observabilidade

Metadados usam compare-and-swap por conta/hash. A renovação tem lease próprio e
barreira persistida antes do HTTP. Outra réplica não pode publicar ignorando essa
barreira. Uma queda ou resposta perdida deixa refresh_uncertain, sem repetição
cega e sem apagar o bloqueio com um recibo antigo. Trocar o segredo no ambiente
cria outro hash e não herda metadados do token anterior. Não há transação SQL
aberta durante a chamada externa.

Readiness recalcula vencimento e idade da prova em cada consulta. Expiração de
acesso a dados também limita a validade. Aviso abaixo de sete dias não antecipa
indevidamente o vencimento. Alertas locais usam categorias fixas e deduplicação
por mudança de estado/hora. A entrega externa desses alertas pertence à fase 6.

Erros Graph 102/190 e 10/200–299 são classificados sem mensagens externas. Erros
de autenticação/permissão suspendem novos envios; uma tentativa já iniciada
continua sujeita à reconciliação conservadora do ADR-010.

Credenciais não têm Serialize ou Debug aberto. O logger só admite targets stories_,
mesmo com RUST_LOG=trace, impedindo diagnóstico de dependências que imprima query
ou cabeçalhos. Retries transparentes de reqwest foram desativados no transporte
compartilhado; a política da aplicação controla novas tentativas.

Referências oficiais consultadas em 17/11/2026:

- https://developers.facebook.com/docs/graph-api/reference/debug_token
- https://developers.facebook.com/documentation/facebook-login/guides/access-tokens/get-long-lived
- https://developers.facebook.com/documentation/instagram-platform/instagram-api-with-instagram-login/business-login
- https://developers.facebook.com/documentation/instagram-platform/instagram-api-with-instagram-login/get-started
- https://developers.facebook.com/docs/graph-api/guides/error-handling
