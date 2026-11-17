# ADR-012: validade e renovação dependem do tipo de token

Data: 17/11/2026. Estado: decisão para o próximo incremento da fase 5;
gerenciamento automático ainda não implementado.

O blueprint generaliza token de 60 dias, renovação em 30 e alerta abaixo de sete.
A documentação oficial atual exige distinguir fluxos:

- Facebook User: token longo geralmente dura cerca de 60 dias; emissão/troca
  exige App ID, App Secret e token válido. Token expirado exige novo login.
- Facebook Page: um token longo derivado de User longo pode não ter vencimento
  fixo. Ele continua sujeito a revogação e outras invalidações.
- Instagram Login: token User longo vale 60 dias e tem endpoint próprio de
  renovação, graph.instagram.com/refresh_access_token.

Portanto não aplicaremos uma troca Facebook User a um Page token, nem usaremos
ig_refresh_token com Facebook Login. Manteremos Facebook Login como padrão atual;
a futura implementação deve inspecionar tipo, validade, permissões e datas efetivas
antes de decidir renovação, alerta ou exigência de reautenticação. Datas de acesso
a dados e validade do token também não são intercambiáveis.

O endpoint oficial debug_token fornece metadados como tipo, is_valid, expires_at,
data_access_expires_at e scopes, sujeito às credenciais de inspeção adequadas.
expires_at=0 não autoriza assumir disponibilidade permanente: erros e revogação
precisam bloquear novos envios. O preflight de cota/Stories atual detecta falhas
básicas de acesso, mas ainda não faz essa inspeção de ciclo de vida.

A alternativa mais próxima do blueprint é renovar no dia 30 somente o fluxo que
oferecer esse mecanismo; nos demais, monitorar metadados e exigir rotação/login
quando aplicável. Não inventaremos uma extensão de validade para tokens de Página.
Segredos de inspeção e troca continuarão vindos do ambiente. O desenho da rotação
precisa preservar esse requisito também após reinício; não será acrescentado token
em texto aberto ao Postgres, logs ou auditoria.

Fontes oficiais consultadas em 17/11/2026:

- https://developers.facebook.com/documentation/facebook-login/guides/access-tokens/get-long-lived
- https://developers.facebook.com/docs/graph-api/reference/debug_token
- https://developers.facebook.com/documentation/instagram-platform/instagram-api-with-instagram-login/business-login
