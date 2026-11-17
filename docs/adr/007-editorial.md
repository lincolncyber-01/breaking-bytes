# ADR-007: verificação conservadora e revisão editorial persistente

Estado: implementado; aceite de fidelidade com modelo real pendente.

N1 exige fonte configurada como primária ou peso 5, data confirmada na origem e
conteúdo recente. Sinais C/Hacker News e indícios de rumor permanecem N3. N2 exige
domínios registráveis distintos pela Public Suffix List, artigos distintos,
trechos distintos e `origin_group` explícito, diferente e normalizado por origem
editorial. Sem identidade editorial cadastrada, duas URLs não autorizam N2.
O operador deve identificar agências, republicações e grupos de mídia; hashes e
domínios, sozinhos, não demonstram independência ou detectam paráfrases sindicadas.

RSS/sitemap são confirmados por uma nova leitura da página canônica e
`article:published_time`; `updated`, `lastmod` e detecção não substituem publicação.
Sem metadado explícito a evidência fica indisponível, mesmo que o feed pareça novo.
APIs GitHub/SEC/Statuspage são relidas e precisam devolver o mesmo artigo e a
data original compatível em até cinco minutos. GitHub restringe proprietário e
repositório. Shortlinks `stspg.io` só passam após confirmação pela API de status
configurada. O material usado e a data confirmada ficam em `assessment_json`.

N3 é reavaliado a cada 60 segundos, até `max_age_hours`, sem gastar tentativas
editoriais. Datas antigas não ganham elegibilidade com a chegada de um novo feed.
A redação permite uma tentativa inicial e até duas reescritas, registradas no
evento. O motivo da falha é dado estruturado da nova tentativa. A checagem usa
outra requisição, outro prompt fixo e nenhuma conversa compartilhada.

O JSON é validado por schema fechado e regras locais de tamanho, atribuição,
números, versões, moedas, datas e entidades. A extração de nomes usa padrões
determinísticos e o campo estruturado `entities`; isso tem limitações para nomes
ambíguos ou omitidos pelo modelo. Relações, voz ativa, neutralidade, tradução e
omissões semânticas exigem a auditoria independente e avaliação humana. Não se
declara garantia universal de fidelidade a partir desses padrões.

Cada segmento precisa de cobertura por afirmações sustentadas e citações
literais de artigos conhecidos. Uma citação existente não prova, por si só,
implicação semântica: a segunda passagem do modelo e o corpus rotulado continuam
obrigatórios. Conteúdo das matérias fica no JSON de usuário, separado dos dois
prompts de sistema. Ferramentas nunca são oferecidas ao modelo.

Confirmações usam token do lease, prazo pelo relógio do banco, versão editorial
e revisão das evidências. Mudanças em associações, matérias ou identidade de
fontes invalidam decisões em voo. Evento já verificado vira SUPERSEDED e recebe
substituto CLUSTERED; decisões antigas não são reutilizadas. CHECKED e a outbox
de renderização são atômicos, com chave `event_id:revision`. O renderizador da
fase 4 deverá conferir estado e revisão antes de consumir esse trabalho.

O blueprint não fornece tamanho de corpus nem percentual numérico de fidelidade.
Adotamos como gate inicial zero falsas aceitações e 100% de concordância em um
corpus rotulado pelo operador, com revisão humana dos casos aceitos e ampliação
para notícias reais. Os oito exemplos sintéticos versionados são apenas smoke
tests. `--evaluate-editorial` mede candidatos rotulados; ele não mede sozinho a
qualidade da geração livre. A geração também precisa de amostragem do pipeline.
Nenhum resultado do modelo real foi obtido nesta implementação.
