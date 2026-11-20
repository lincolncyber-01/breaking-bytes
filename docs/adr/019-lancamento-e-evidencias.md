# ADR-019: lançamento por digest e aceite temporal explícito

Data: 20/11/2026. Estado: implementado; aceites externos pendentes.

## Decisão

O destino continua sendo VPS Linux x86-64 com Docker Compose, conforme o
blueprint. Dev usa o workspace; staging tem projeto, volume PostgreSQL, bucket,
token administrativo e conta Instagram distintos. A CLI exige intenção explícita
para publicar em staging e recusa a identidade da conta de produção.

O workflow de release exige sete checks bem-sucedidos do mesmo commit em main,
versão semântica igual ao workspace e tags ainda inexistentes. Constrói e audita
as duas imagens, verifica HIGH/CRITICAL e segredos e produz referências por
digest. Não usa latest. Um helper de credenciais somente leitura recebe o token
pelo ambiente; o config temporário do Docker contém apenas o nome do helper.
Publicações das duas imagens não são uma transação: falha parcial exige nova
versão após investigação, sem sobrescrever tags já publicadas.

O rollout guarda snapshot não secreto da configuração, checksum e identidades
da conta/bucket; pausa com auditoria, para o worker, migra e valida readiness,
modo, teto e identidade da imagem executada. Exige daemon local e arquitetura
amd64, porque foram os ambientes ensaiados. Rollback reutiliza a imagem anterior
em cache, conserva pausa e não restaura o banco. Migrations precisam continuar
compatíveis com a versão anterior. Se não for possível confirmar a pausa no
banco, a recuperação não reinicia o worker anterior automaticamente.

## Evidências e limitações

O observador exige 24 h de coleta, 48 h de dry-run, 72 h de canário a 10/dia e
72 h a cada promoção de teto 20/40/80. Não existe argumento CLI para encurtar a
janela. Amostra readiness, contadores duráveis, fontes e VmHWM do processo real;
mudança de processo, teto ou implantação invalida o aceite. A aprovação vincula
hashes dos relatórios, consulta SQL somente leitura e atestado humano de
fidelidade, deduplicação, staging, alertas, restauração, kill switch e memória
sob carga. Um recibo local não é uma assinatura independente nem prova que um
operador desonesto revisou conteúdo. Proteja o diretório e as permissões da VPS.

Testes com relógio injetado verificam as regras, não representam dias reais de
observação. Os contratos de rollout verificam ordenação/falhas; PostgreSQL e
psql são reais no teste de relatório/pausa. Execução de workflow manual,
registry real, VPS, Meta, R2, LLM, Telegram e janelas temporais reais continuam
dependendo das contas e infraestrutura do proprietário. Nenhum recibo aprovado
fictício é distribuído.

Os snapshots visuais já comparavam seis JPEGs pixel a pixel. Insta 1.49.0 foi
acrescentado para registrar também o layout estruturado, como exige o blueprint.
Os valores vieram das referências JSON existentes; não se aprovou uma mudança
visual regenerando silenciosamente os JPEGs.

Disponibilidade observada >=99,5% numa janela de lançamento não comprova SLO
mensal. A consulta de detecção considera fontes primárias elegíveis a N1; a
latência de publicação considera apenas N1 realmente publicados. Corpus,
semântica e representatividade das amostras exigem revisão humana.
