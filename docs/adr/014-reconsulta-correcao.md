# ADR-014 — Reconsulta de fontes e correção confirmada

Data: 17/11/2026. Estado: aceita para implementação; sem habilitação em produção.

O blueprint, seção 7.9, exige reconsultar as fontes de Stories publicados nas
últimas 24 horas, ampliar a janela de alto impacto e publicar CORREÇÃO quando
confirmada. A classificação semântica não pode ser inferida por mudança de hash,
HTTP 404, nova data, palavra isolada ou instrução contida na matéria.

## Decisão

Cada reserva de publicação conserva draft e assessment em `editorial_snapshot`.
Após `published`, `published_unresolved` ou `dry_run`, um acompanhamento durável
registra as fontes efetivamente usadas. O padrão é 24 horas, estendido a 72 horas
quando score >= 0,8. A duração maior e o limiar são suposições configuráveis;
o blueprint não fixa esses valores. No máximo quatro fontes por Story, por job
serial, usando o transporte compartilhado e as barreiras SSRF/robots/tamanho.

Reconsulta conserva a data original exposta pela fonte e separa `observed_at`.
Uma matéria publicada há vinte horas pode sustentar sua própria correção recente;
o sistema não troca sua data pela hora da coleta. A elegibilidade da revisão
exige baseline de publicação, origem ativa e idêntica, hash atual e observação
recente. N1 exige fonte primária, N2 duas origens independentes. Ausência de data
atual na página não apaga a data congelada do Story nem inventa data nova; o
baseline já foi aprovado na publicação original. A revisão registra essa ausência.

A comparação normaliza somente NFC e espaços, preservando sinais numéricos e
pontuação. Nota explícita encontrada no rodapé pode ser incluída no trecho
limitado a 2.000 caracteres; a nota tem limite de 500. Isso é sinal para revisão,
nunca autorização para publicação. Scripts e elementos não editoriais são excluídos.
Conteúdo fora dos trechos capturados continua sendo uma limitação de detecção.

Um parecer estruturado adicional classifica unchanged, uncertain, update,
correction ou retraction. Decisões publicáveis precisam relacionar segmento do
Story anterior, citação literal anterior e citação nova ausente dos baselines.
Correção/retratação também exige nota editorial explícita citada. O candidato
passa novamente pelo pipeline; uma segunda chamada independente confirma a
classificação além de checar cada afirmação. Esse custo adicional de LLM é
necessário para evitar transformar toda alteração textual em correção. Saídas
inválidas têm tentativas limitadas; sem confirmação, nenhuma publicação é criada.

O tipo editorial é explícito no evento, preservado nas reescritas e na reserva.
Correções usam os slots reservados da política existente. Retratação usa a
etiqueta CORREÇÃO e texto sustentado pela nota; não existe endpoint inventado
para apagar ou editar um Story já publicado. O evento original real pode entrar
em NEEDS_CORRECTION; em simulação ele permanece QUEUED.

Snapshots, versão do acompanhamento, decisão, evento e outbox são persistidos
com transações curtas e fencing de lease. Mudança de fonte durante LLM, render
ou agendamento invalida a prova anterior. Uma revisão aceita não é repetida para
o mesmo conjunto de hashes. Sob o lock da conta, uma reserva pendente ou incerta
de outra revisão do mesmo Story bloqueia a próxima reserva até sua reconciliação. Após a publicação terminal, o acompanhamento
anterior é substituído pelo do novo Story, prevenindo correções paralelas.
A troca entre simulação e modo real não promove candidatos antigos.

## Limites e consequências

Ativação padrão é false, inclusive no Compose. A reconsulta pode operar sem
LLM, deixando a revisão pendente. A pausa global vale para ambos os workers.
Falhas integrais de fonte recebem recuo exponencial com jitter e teto de uma
hora; captura inválida é falha isolada da fonte, sem renovar sua prova nem encerrar
o acompanhamento das fontes saudáveis; jobs esgotados continuam visíveis para investigação, sem ressurreição cega.

Publicações legadas sem snapshot não recebem cópia inferida dos artigos atuais.
É impossível reconstruir honestamente o texto que foi publicado somente com o
estado mutável atual; esses registros exigem investigação manual.

A confirmação semântica continua dependente do modelo e precisa de corpus real
e staging. As verificações determinísticas rejeitam citações falsas e prova
obsoleta, mas não demonstram a relação semântica sozinhas. Não houve publicação
real, avaliação de acurácia em corpus real ou validação de memória da reconsulta
sob carga nesta decisão. O caos local não substitui a interrupção de processo
em staging exigida pelo portão completo da fase 5.
