# Publicação e simulação

Aplique a migration 0005 antes de iniciar o novo binário. Mantenha dry_run=true
até revisar os portões pendentes em docs/reports/phase-5.md. O padrão habilita o
worker de publicação, mas não exige credenciais Meta/R2 em dry_run.

O caminho local é RENDERED → reserva QUEUED → verificação do JPEG → status
dry_run. Não há upload nem chamadas Graph nesse caminho. O evento não recebe
PUBLISHED artificialmente. A unicidade impede que trocar dry_run promova a mesma
intenção; um teste real exige candidato novo, renderizado no modo correspondente.

No modo real, a configuração aceita somente referências de ambiente:
IG_USER_ID, IG_ACCESS_TOKEN, R2_ACCOUNT_ID, R2_ACCESS_KEY_ID e R2_SECRET_ACCESS_KEY.
LLM_API_KEY é obrigatório somente se a redação estiver habilitada. O cliente usa
Graph v27.0 e o host correspondente a instagram.login. Verifique token, conta,
permissões e App Review no painel Meta; HTTP 200 de leitura não prova permissão de
publicação nem a elegibilidade completa da conta para Stories.

Facebook Login: instagram_basic, instagram_content_publish e pages_read_engagement;
papéis concedidos pelo Business Manager podem exigir permissões adicionais.
Instagram Login: instagram_business_basic e instagram_business_content_publish.
A listagem de Stories no fluxo escolhido precisa ser validada com a conta real.
As referências oficiais estão no ADR-010; o guia e as referências divergem também
na descrição do tipo de token Facebook. Valide o token emitido para a sua integração.

O preflight consulta a cota, Stories ativos e faz sonda reversível R2. Sem sucesso,
novos jobs não são consumidos e o modo real não tem readiness. A sonda é repetida
a cada minuto. publishing_ready indica apenas o resultado desse preflight; não
substitui os portões de implantação. dry_run pode ter readiness local sem rede.

O processamento persiste objeto, container, prontidão e início do envio em etapas.
O HTTP tem limite de dez segundos e respostas limitadas; redirect não é seguido
pelo transporte compartilhado. Containers em processamento são consultados a cada
minuto. Falha antes da barreira pode ser retomada. Depois dela, apenas reconciliação.

Se ocorrer uncertain:

1. Pause novos envios com PUT /admin/pause autenticado.
2. Identifique publication_id, container_id e request_started_at no banco. Não
   copie tokens, URLs assinadas ou respostas completas para tickets/logs.
3. Confira status_code do container e Stories da conta pelo painel oficial.
4. Aguarde a reconciliação se o status remoto ainda puder mudar. PUBLISHED leva
   a published_unresolved quando a resposta original foi perdida.
5. Mantenha incertos sem repetição automática e investigue a identidade da mídia
   antes de qualquer intervenção no banco. Não redefina request_started_at,
   container_id ou dedupe_key para forçar uma segunda publicação.

A resolução está disponível em POST /admin/publications/ID/resolve, sob pausa,
com recibo UUID e atestado explícito da associação container/mídia. Ela não cria
Story. Consulte [operations.md](operations.md) antes de intervir; sem prova,
conserve a incerteza. Não faça reparos diretos na intenção por SQL.

A limpeza R2 alcança no máximo vinte objetos terminais por ciclo, após TTL de
segurança contado da confirmação ou criação, o que for mais recente. Objetos
incertos permanecem disponíveis. Configure lifecycle de um dia somente para
health/. Artefatos locais seguem a retenção documentada no runbook.

O ADR-012 registra a diferença entre validade de Facebook User, Facebook Page e
Instagram Login. Não aplique a regra genérica de 60 dias a todos os tokens.

Preflight e limpeza têm orçamento total de vinte segundos por rotina e podem ser
interrompidos pelo sinal de parada. Queda durante limpeza deixa o objeto elegível
para nova remoção idempotente no ciclo seguinte.

A migration 0006 adiciona controle obrigatório de token no modo real. Consulte
docs/tokens.md e ADR-013: o preflight agora inclui o monitor, e a readiness recusa
credenciais vencidas, antigas ou com rotação pendente.
