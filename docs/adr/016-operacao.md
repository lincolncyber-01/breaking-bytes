# ADR-016 — métricas, alertas, privilégios e restauração

Data: 17/11/2026. Estado: implementação em validação; serviços reais pendentes.

A stack de observabilidade usa metrics 0.24.6 e metrics-exporter-prometheus 0.18.3,
versões estáveis consultadas no crates.io. O exporter não abre outro listener:
Axum serve `/metrics` na rede privada. Histogramas LLM/render são registrados no
recorder local, sem atravessar await; upkeep a cada cinco segundos limita buffers.
Contadores de negócio e histogramas de detecção/publicação são persistidos no
Postgres dentro da transação que confirma seu efeito. A resposta combina essas
séries duráveis com o exporter. As labels são códigos controlados, camada, nível
ou ID de fonte; nunca URL, título, corpo, token ou mensagem do provedor.
As séries duráveis começam na migration 0009. Não há backfill que invente tempos
históricos. Tempos de publicação consideram confirmação remota; simulação não é
publicação real. Histogramas runtime reiniciam com o processo.

Telegram foi escolhido entre as alternativas do blueprint. Endpoint e formato
foram conferidos em https://core.telegram.org/bots/api#sendmessage.
O bot e o chat vêm somente do ambiente. Alertas têm condições persistidas, lease,
Retry-After, backoff e deduplicação. Uma entrega pode repetir após perda de resposta:
o Telegram não fornece a idempotência necessária para prometer exatamente uma
mensagem. Essa limitação não altera a barreira dos Stories. Bloqueios factuais
individuais não são descartados enquanto não forem enviados. Pausa suprime
alertas de coleta esperada, mas não impede alertas de credencial e backup.

O processo de publicação exige papel PostgreSQL sem DDL e sem UPDATE/DELETE na
auditoria. O bootstrap explícito cria stories_owner, stories_runtime e
stories_backup; transfere apenas objetos conhecidos do projeto e conserva a
extensão vector sob administração. As senhas são variáveis de ambiente e os
comandos de provisionamento desabilitam logs de instruções/erros daquela sessão.
SQLx 0.9 exige AssertSqlSafe para DDL dinâmico: seu uso ficou restrito a comandos
produzidos por format(%I/%L) no servidor com identificadores fixos e senhas
hexadecimais validadas. O runtime não tem acesso a esse comando via HTTP.

Backup e restauração são ferramentas operacionais separadas, implementadas com
Python padrão e as CLIs oficiais pg_dump/pg_restore e AWS CLI. O pipeline permanece
inteiramente Rust e o workspace mantém os doze crates. Evitamos colocar dump,
hash de arquivo grande ou ferramentas de administração dentro do runtime Tokio.
A AWS CLI 2.39.6 foi conferida na documentação oficial; a imagem é fixada por digest.
https://developers.cloudflare.com/r2/examples/aws/aws-cli/
https://docs.aws.amazon.com/cli/latest/reference/s3api/put-object.html
Credenciais chegam por ambiente, sem perfis em disco, senhas em argv ou stderr
externo nos logs. Arquivos privados contêm o dump e seu manifesto, não credenciais.
Upload só é confirmado após GET e SHA-256; retenção mantém sete períodos diários
e quatro semanais, com um arquivo concluído por período. Namespace próprio,
manifestos imutáveis por hash e conferência antecedem qualquer coleta de lixo.
Órfãos sem manifesto só são elegíveis após 48 h, além do prazo máximo da operação.

A restauração exige banco vazio, nome terminado em `_restore_check` e identificador
de cluster diferente do registrado no manifesto. O servidor de teste usa rede
interna e volume descartável próprios. Após restaurar, o banco fica pausado e uma
barreira SQL impede retomada por 24 h; watches antigos expiram. Um dump diário
possui RPO de até 24 h: não recupera intenções que ainda não estavam no snapshot.
A quarentena e o isolamento evitam usar esse ledger como se fosse atual.
Em recuperação de desastre, é obrigatório primeiro interromper e isolar o servidor
antigo, conferir o ledger e os Stories manualmente e preservar incertezas.
Não há promessa de RPO zero ou failover automático.

Timings assumidos: fonte silenciosa 10 min; ausência de notícia 2 h; p95 real em
janela aproximada de 5 min acima de 300 s; três resultados de publicação seguidos
com falha/incerteza; aviso de token conforme warning_days (padrão 7); backup após
30 h e restauração após 35 dias. Alertas de qualidade usam o ID do evento.
Os timers são meia-noite e dia 1 às 01h, America/Bahia. O provedor de segredos deve
reapresentar o ambiente dos serviços após cada boot; ausência de variável falha
fechado e não cria arquivos de segredos.

Ferramentas de segurança: cargo-deny 0.21.0, cargo-audit 0.23.0 e Trivy 0.76.2,
consultadas nas publicações oficiais. Os três avisos informativos de manutenção e a retirada de OpenSSL descobertos pelo scan estão no ADR-017; nenhuma vulnerabilidade é excluída. Licenças de
crates privados não publicados não são atribuídas automaticamente ao proprietário.
Fuzz usa nightly fixo somente na ferramenta; o produto compila em Rust estável.
O fallback hash/entidades continua explícito conforme ADR-005 e a opção permitida
na seção 2 do blueprint. O backend fastembed e a avaliação semântica continuam
pendentes; não se apresenta hash como embedding. A consulta atual encontrou
fastembed 7.7.1 e nenhum ort estável no campo max_stable_version do registry.

UUID foi atualizado de 1.29.2 para o patch estável 1.29.3 descoberto nesta fase.
O lock e a compilação registram a resolução efetivamente utilizada.
