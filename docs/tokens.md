# Validade e operação de tokens

Aplique a migration 0006 antes de atualizar o binário. O modo padrão continua
`dry_run=true`: ele não exige credenciais Meta, não inspeciona nem renova tokens.
O status administrativo retorna token_health=null nesse modo, sem simular prova.

No modo real, a inspeção/controle de validade é obrigatório. A configuração fica
em instagram.token. Os padrões são checagem a cada 300 segundos, aviso abaixo de
sete dias e tentativa de renovação Instagram após 30 dias. Cada novo envio também
consulta o estado persistido; a readiness verifica vencimento em cada requisição.
Ela recusa metadados antigos mesmo se o último preflight tiver sido bem-sucedido.

## Facebook Login

Forneça IG_USER_ID e IG_ACCESS_TOKEN, além de META_APP_ID e META_INSPECTION_TOKEN.
Este último deve ser uma credencial de inspeção admitida para o mesmo aplicativo;
ele nunca é usado para publicar. Quando o resultado de debug_token tiver tipo PAGE,
forneça também FB_PAGE_ID. O identificador é comparado com profile_id, não com o
IG_USER_ID. USER e PAGE têm políticas distintas; tipos não reconhecidos bloqueiam.

O monitor exige instagram_basic, instagram_content_publish e pages_read_engagement.
Papéis do Business Manager e elegibilidade de publicação podem exigir requisitos
adicionais: consulte o ADR-010 e valide a conta em staging. Inspeção de token não
substitui App Review ou autorização de publicação da Página.

Tokens longos de Página podem não ter vencimento fixo, mas continuam revogáveis.
Tokens User usam o prazo retornado pela Meta. Expiração do token e acesso a dados
são tratados separadamente. Não há renovação automática genérica de Facebook
User/Page: obtenha a credencial pelo fluxo oficial e faça a rotação de ambiente.

## Instagram Login

A renovação documentada é suportada para tokens longos válidos com ao menos um
dia de idade. Como não usamos um endpoint de introspecção não confirmado, o
operador precisa fornecer IG_TOKEN_RECEIPT com as datas e permissões da emissão.
O recibo não contém o segredo e precisa corresponder ao seu hash BLAKE3.

Após definir IG_ACCESS_TOKEN no ambiente, gere o hash sem exigir banco ou Admin:

```sh
cargo run --locked --bin stories-tech -- --token-fingerprint
```

Defina IG_TOKEN_ISSUED_AT e IG_TOKEN_EXPIRES_AT com os instantes reais da emissão,
em RFC 3339, e IG_TOKEN_SCOPES com as permissões efetivamente concedidas, separadas
por vírgula. Não deduza o prazo de um token curto a partir da regra de 60 dias.
O recibo aceito é de token longo, com janela superior a um dia e até 60 dias.
Para construir o JSON completo a partir desses valores:

```sh
export IG_TOKEN_FINGERPRINT="$(cargo run --locked --bin stories-tech -- --token-fingerprint)"
export IG_TOKEN_RECEIPT="$(python3 -c 'import json, os; print(json.dumps({"fingerprint": os.environ["IG_TOKEN_FINGERPRINT"], "issued_at": os.environ["IG_TOKEN_ISSUED_AT"], "expires_at": os.environ["IG_TOKEN_EXPIRES_AT"], "scopes": os.environ["IG_TOKEN_SCOPES"].split(",")}))')"
```

As permissões mínimas são instagram_business_basic e
instagram_business_content_publish. O estado identifica a origem oauth_environment;
esses metadados declarados não são prova de concessão remota. Cota/Stories são
consultados no preflight, e a publicação precisa do teste real de staging.

Quando a renovação conserva o valor do token, o monitor guarda somente o novo
prazo e o horário de confirmação. Um reinício preserva esses metadados pelo hash,
mesmo que o recibo inicial tenha datas antigas. O segredo continua vindo do ambiente.

Se vier outro token, ou se a resposta da renovação se perder, novos envios ficam
bloqueados. O daemon não escreve nem revela a nova credencial. O operador deve
obter uma credencial válida pelo fluxo Meta, atualizar o ambiente e recriar o
processo. Não altere o hash ou remova a barreira por SQL para contornar o estado.
A integração com um serviço externo de rotação de segredos ainda não existe.

## Diagnóstico e recuperação

GET /admin/status, autenticado, inclui token_health com status, tipo, origem,
dias restantes, horário de checagem/renovação e categoria de falha. Ele não inclui
token, hash, recibo bruto ou credencial de inspeção. Estados valid e expiring podem
permitir envio; expired, invalid, stale e rotation_required bloqueiam. A readiness
ainda depende de banco, preflight Meta/R2 e habilitação do publicador.

O primeiro aviso de uma categoria e mudanças de categoria são registrados; repetições
são limitadas a uma por hora. Alertas enviados para serviços externos serão
implementados e validados na fase 6. Consulte audit_log para token_refresh_started
e token_refresh_result, que só contêm metadados controlados.

Para rotação, atualize os valores no mecanismo de ambiente da VPS/implantação e
recrie o serviço. No Compose, após atualizar seu arquivo local de ambiente:

```sh
docker compose --env-file .env -f deploy/docker-compose.yml up -d --force-recreate app
```

A aplicação não atualiza esse arquivo. Não cole o token na Admin API, em logs ou
em tickets. RUST_LOG seleciona eventos do próprio projeto; logs de dependências
são suprimidos para evitar query/cabeçalhos com credenciais.

A fase 5 continua aberta: validação com Meta real, rotação externa de um valor
alterado, correções automáticas e resolução administrativa de publicação incerta
permanecem pendentes. Leia o ADR-013 para as decisões e limitações.
