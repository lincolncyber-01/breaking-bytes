# ADR-006: coleta pública com falha segura e polling

Data: 16/11/2026. Estado: implementado; observação operacional pendente.

Contratos consultados em fontes oficiais:

- GitHub Releases: https://docs.github.com/en/rest/releases/releases#list-releases
- Hacker News Firebase: https://github.com/HackerNews/API
- SEC: https://www.sec.gov/search-filings/edgar-search-assistance/accessing-edgar-data
- Statuspage público: https://www.githubstatus.com/api

RSS e Atom usam feed-rs 3.0.0; sitemaps usam quick-xml 0.42.0. As APIs vigentes
foram verificadas ao compilar. XML tem limites de bytes, nós e profundidade;
DOCTYPE é recusado. Sem data confiável, o item é descartado. lastmod de sitemap
não é tratado como data de publicação. Sitemaps sem título ou data usam metadados
HTML, incluindo canonical, antes do filtro temporal final.

O coletor GitHub lê releases públicos; HN expande IDs em itens e ignora entradas
apagadas ou mortas; Statuspage lê incidentes e atualizações; EDGAR aceita Atom
com User-Agent de contato obrigatório. Requisições à SEC compartilham espaçamento
de 200 ms, abaixo do máximo documentado de 10 requisições por segundo.

Um único reqwest::Client é compartilhado. Conexão tem timeout de 3 s, requisição
10 s, no máximo três redirecionamentos e limite de resposta. Há semáforo por
origem, limite de origens e limite separado para robots.txt. Parsing e regras
robots executam em spawn_blocking com concorrência limitada. DNS só aceita
endereços públicos, rejeitando respostas mistas, redes especiais e IPs literais
privados. O resolver usado pela conexão aplica a validação, protegendo contra
rebinding entre checagem e conexão.

Redirecionamentos de conteúdo entre origens são recusados: a URL final deve ser
cadastrada explicitamente. Na mesma origem, cada destino é checado contra robots.
robots.txt indisponível por 5xx bloqueia a coleta; 401/403 também bloqueiam.
O cliente não envia segredos, e erros não incluem URLs ou corpos externos.

Um proxy de ambiente faz a resolução fora do processo e quebra a garantia acima.
O cliente recusa iniciar nessa configuração. Não desativamos o proxy da Cloud
para contornar a política do ambiente. Os testes usam apenas uma origem loopback
fixa em código compilado exclusivamente com cfg(test), sem opção de liberar redes
privadas no binário. A coleta real deve ocorrer em VPS/egress validado.

Polling persiste ETag, Last-Modified, falhas e próxima tentativa no banco. Circuit
breaker abre após falhas repetidas e faz reteste com backoff; Retry-After e jitter
limitam pressão. WebSub fica adiado porque não há endpoint público TLS validado
para callbacks nesta fase. O polling de 60–120 s é a alternativa próxima prevista
na captura; suporte WebSub e teste do callback permanecem pendentes.

Não há conta social, agência licenciada ou conteúdo protegido sendo raspado.
As duas fontes de exemplo são oficiais e retornaram HTTP 200 via curl no ambiente.
Isso verifica os endpoints pelo caminho do proxy, sem alegar coleta real contínua
pelo cliente Rust nem autorização geral para reutilizar imagens ou textos.
