# ADR-011: R2 assinado e prioridade conservadora

Data: 17/11/2026. Estado: implementado; compatibilidade real e calibração pendentes.

A documentação Cloudflare recomenda aws-sdk-s3 com região auto e endpoint da
conta. Fixamos 1.159.0, versão estável consultada no registry. Não usamos
aws-config, cadeia padrão de credenciais ou IMDS. O SDK apenas assina operações;
PUT, GET de conferência e DELETE usam o cliente reqwest compartilhado. As APIs
foram compiladas e exercitadas contra servidor HTTP simulado. O teste de assinatura
verifica host, região, validade e parâmetros SigV4 sem chamar a Cloudflare.

A conta R2 é uma referência de ambiente validada como 32 caracteres hexadecimais;
ela não pode fornecer endpoint arbitrário. O bucket é configurável. URLs assinadas
são capacidades temporárias: não têm Debug aberto nem são persistidas. Uma chave
determinística por publicação/hash torna PUT repetido idempotente. O GET de
conferência verifica os mesmos bytes antes de expor o endereço à Meta.

O preflight faz sonda reversível de escrita/leitura/remoção sob health/. Configure
regra de lifecycle de um dia para esse prefixo: queda antes da remoção pode deixar
uma sonda órfã. Não aplique essa regra aos objetos stories/ incertos.

A política usa os pesos 0,30/0,20/0,20/0,15/0,15 do blueprint, reserva configurável
padrão 5, cota padrão 40 e máxima 80, e intervalo padrão 90 segundos. Breaking a
partir de 0,80 e correções bypassam o intervalo, preservando o limite total.
Perto do limite, a pontuação mínima comum cresce de forma determinística.

O blueprint não fornece um modelo calibrado para impacto, alcance e urgência.
O worker adota impacto=0,5, alcance=0,5 e urgência=0, sem classificação breaking
automática; novidade vale 1 para new e 0,5 para update. Confiabilidade deriva do
nível verificado (N1=1, N2=0,8, outros=0,5). Sinais e score ficam auditados.
Isso é uma aproximação explícita, não uma avaliação semântica de relevância.
A política pura aceita sinais calibrados, mas sua obtenção automática ainda exige
corpus e implementação posterior dentro da fase 5.

A criação automática de correções, a reconsulta das fontes publicadas por 24 horas
e o gerenciamento de expiração/renovação do token ainda não integram este
incremento. A fase 5 continua aberta; não se deve tratar o preflight como aceite
completo do fluxo OAuth ou como prontidão para produção.

Referências:

- https://developers.cloudflare.com/r2/examples/aws/aws-sdk-rust/
- https://developers.cloudflare.com/r2/api/s3/presigned-urls/
