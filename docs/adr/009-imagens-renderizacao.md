# ADR-009: imagens revisadas, arte original e template determinístico

Estado: implementado. A detecção automática de marca d'água/relevância foi
substituída explicitamente por revisão vinculada ao arquivo, conforme explicado
antes da implementação desta fase.

O blueprint exige detectar marcas d'água, imagens genéricas e imagens sem relação
com a notícia. Ele não especifica um modelo visual nem um corpus calibrado. Não
há credencial disponível para validar um serviço visual e uma heurística local
não comprova ausência de marca ou pertinência semântica. Não implementamos nem
declaramos um detector automático aprovado.

A alternativa adotada é falhar de forma conservadora: imagem externa exige
domínio permitido, licença explícita, referência dos direitos, crédito, origem
editorial compatível, revisor identificado, data da revisão e hash BLAKE3 exato.
O revisor precisa declarar ausência de marca d'água, relevância e ausência de
conteúdo genérico. A revisão expira em 180 dias e qualquer mudança de bytes a
invalida. O manifesto é configuração confiável versionada pelo operador; não é
preenchido por matérias, feeds ou pelo LLM. Essa revisão humana não equivale a
detecção automática. Um detector futuro precisa de avaliação própria antes de
substituir esse controle.

Preferência: press kit, fonte primária, og:image da matéria e banco licenciado.
São feitas até quatro tentativas, em ordem de preferência, para limitar a espera
de rede. O og:image só participa se estiver no manifesto revisado e corresponder
à matéria. Sem imagem elegível, com download inválido ou com crédito que não
caiba, o sistema usa arte tipográfica original feita com tiny-skia e o título
já aprovado. Essa é a opção de arte gerada prevista no blueprint, sem serviço
externo de geração. Não há busca irrestrita em bancos de imagem nesta fase.

JPEG, PNG e WebP são reconhecidos pelos bytes e confrontados com Content-Type.
O padrão é até 8 MiB, 16 milhões de pixels e 12000 px por dimensão; largura
mínima de 1000 px. Dimensões são lidas antes do bitmap; o decoder recebe limite
de alocação de 96 MiB. A orientação EXIF é aplicada e os metadados não seguem
para o JPEG final. Downloads reutilizam o cliente HTTP, com DNS público, robots,
timeout de 10 s, conexão de 3 s e até três redirecionamentos na mesma origem.
Proxies de ambiente continuam bloqueados para URLs jornalísticas e de imagem.

O template mantém canvas 1080×1920, JPEG 90, fundo preto, margens 64/250 px,
imagem de 760 px com até 570 px de altura em y=300 e raio de 12 px. Crédito
fica em y=884; etiqueta opcional em y=928; título em y=980; resumo em y=1230;
a linha de fonte tem baseline y=1640. As posições não definidas pelo blueprint
foram fixadas aqui para tornar a composição reproduzível.

Usamos Inter Regular/Medium/SemiBold 4.1, a versão atual do repositório oficial
rsms/inter, embutida no binário. Licença OFL, URL e hashes estão em assets/fonts.
Não carregamos fontes do sistema. Texto sem cobertura de glifos é bloqueado.
Controles de direção bidirecional e espaços invisíveis também são bloqueados,
para impedir que a aparência de nomes ou números divirja do texto checado.
Fonte, tamanho, cor, quantidade de linhas e entrelinha seguem a especificação;
o texto pode reduzir até 10%, em passos de 1%. Não há reticências nem corte.
Se ainda não couber, o evento ganha substituto para nova verificação/redação,
com pedido de título de até seis palavras e duas linhas de até 45 caracteres.
O orçamento global de até duas reescritas é preservado.

O worker serial executa decode, shaping, resize, composição e hash fora do
runtime. O cache de glifos é reiniciado ao ultrapassar 4096 entradas. A imagem
final é gravada em caminho derivado de evento/revisão/hash, com arquivo temporário
exclusivo, sync_all, rename e sync do diretório no Linux. Depois disso, estado
RENDERED, metadados, auditoria e outbox schedule são confirmados em uma transação
com fencing de lease, versão e revisão.

Disco e Postgres não formam uma transação distribuída. Uma queda entre a escrita
e o commit pode deixar arquivo órfão, sem publicação. A retomada produz o mesmo
caminho por hash e só então confirma o banco. A limpeza desses órfãos precisa
respeitar referências e retenção na fase de endurecimento.

Mudanças de evidência também invalidam candidatos RENDERED/QUEUED. Publicação
deverá conferir novamente estado e revisão antes de qualquer chamada externa.
A outbox schedule não tem consumidor de publicação nesta fase. R2, Meta e caos
de publicação continuam na fase 5.

O blueprint não define percentil para a meta de 150 ms. Adotamos p95 abaixo de
150 ms como gate reproduzível, com mediana, execução fria e máximo publicados.
Essa é uma interpretação de SLO para Linux, não um limite de tempo real rígido.
A primeira medição teve um outlier de 413,8 ms; ele não foi removido. Portanto,
o resultado não comprova 150 ms como máximo absoluto de toda execução. A causa
desse pico não foi isolada e a VPS final precisa de benchmark e observabilidade.
