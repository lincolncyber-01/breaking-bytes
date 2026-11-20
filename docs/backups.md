# Backup e recuperação

O código implementa dump noturno, conferência remota por SHA-256, sete datas diárias
e quatro semanas, com teste mensal de restauração. Não houve acesso ao bucket
real neste ambiente. Os testes usam PostgreSQL real e armazenamento de contrato.

## Preparação

Use bucket privado separado `stories-backups` e credencial exclusiva com leitura,
escrita e remoção apenas nesse bucket. Não reutilize a credencial `stories-img`.
Não aplique expiração automática curta aos backups verificados: a retenção só
remove versões antigas depois de confirmar uma nova. Objetos sem manifesto há
mais de 48 h são coletados pelo próprio job após novo backup verificado.

Forneça pelo seu provedor: BACKUP_R2_ACCOUNT_ID, BACKUP_R2_ACCESS_KEY_ID,
BACKUP_R2_SECRET_ACCESS_KEY e BACKUP_DATABASE_URL. A conexão deve usar
stories_backup, banco stories e host postgres na rede Docker de produção.
Não escreva essas variáveis em TOML, scripts, perfis AWS ou arquivos versionados.
BACKUP_BUCKET é não secreto e tem padrão stories-backups.

O bootstrap do banco é `stories-tech --provision-database`, com
BOOTSTRAP_DATABASE_URL do administrador e três senhas hexadecimais distintas,
MIGRATION_DATABASE_PASSWORD, RUNTIME_DATABASE_PASSWORD e BACKUP_DATABASE_PASSWORD,
de 32 a 128 caracteres. Use o provedor para gerar/fornecer os valores. Em Compose,
suba primeiro postgres e execute:

```sh
docker compose -f deploy/bootstrap-compose.yml run --rm provision
```

Depois configure DATABASE_URL com stories_runtime e MIGRATION_DATABASE_URL com
stories_owner. O usuário administrador não participa do processo normal.
O bootstrap pode transferir os objetos conhecidos de instalações antigas; faça
backup e pause antes de trocar papéis de uma instalação já existente.

Construa a imagem operacional ou use a versão publicada pelo release:

```sh
docker compose -f deploy/backup/compose.yml build backup
docker compose -f deploy/backup/compose.yml run --rm --no-deps backup
```

O comando retorna somente chave, hash, tamanho e estado. A conclusão significa
que dump e manifesto foram lidos novamente do R2. O estado local é registrado em
operational_backups. Uma falha não autoriza apagar o último backup bom.
O diretório de trabalho é um volume privado compartilhado pelos jobs, com lock
local; não é tmpfs, para não carregar o dump inteiro na RAM. Arquivos são 0600 e
os diretórios 0700. Após uma queda, a próxima execução exclusiva remove os seus
diretórios temporários anteriores. A garantia de exclusão é para uma VPS.

Limites: BACKUP_MAX_BYTES padrão 512 MiB, configurável entre 1 MiB e 8 GiB;
espaço livre mínimo de duas vezes esse limite mais 64 MiB; prazo total 900 s.
Hash e transferência usam arquivo/streaming. Um tamanho excessivo, checksum
incorreto, erro HTTP ou timeout interrompe a operação. Verifique capacidade e
tempo da VPS com o volume real do banco antes de produção.

## Agendamento e teste mensal

Instale os quatro arquivos deploy/systemd em /etc/systemd/system, com dono root e
modo 0644. O checkout de operação é /opt/breaking-bytes. Configure seu provedor para
entregar as variáveis por ambiente aos serviços em todo boot; PassEnvironment não
persiste segredos através de reinicialização do host. Importe somente os nomes
necessários com systemctl import-environment no contexto desse provedor e ative:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now breaking-bytes-backup.timer breaking-bytes-restore.timer
sudo systemctl start breaking-bytes-backup.service
sudo systemctl start breaking-bytes-restore.service
```

Para o teste mensal forneça também RESTORE_DATABASE_PASSWORD distinta e
RESTORE_DATABASE_URL apontando para restore-db/stories_restore_check. O servidor
não tem porta no host e pertence apenas à rede interna do projeto descartável.
O cliente lê o último manifesto diário concluído, verifica SHA-256 e restaura.
Confere as tabelas, pausa o pipeline, aplica quarentena de 24 h e registra o teste
na origem e no destino. O down --volumes remove somente o projeto
breaking-bytes-restore-check; a rede de produção e o volume de trabalho são externos.

Consulte operational_restore_checks e o journal dos serviços. Logs não incluem
DSN ou saída bruta dos provedores. Depois de 30 h sem backup e 35 dias sem teste,
a observabilidade cria alertas quando backups_expected=true.

## Desastre

1. Interrompa e isole os trabalhadores do host antigo antes de iniciar recuperação.
2. Preserve seus discos e auditoria; registre a chave do backup pelo console R2.
3. Crie outro cluster PostgreSQL 16 com pgvector 0.8.2 e banco vazio com sufixo
   _restore_check. Não forneça credenciais Meta ou de publicação ao servidor.
4. Execute a imagem operacional com modo restore-check e `--key` seguido da chave
   do dump. Forneça RESTORE_DATABASE_URL por ambiente; omita BACKUP_DATABASE_URL se
   a origem não existir. As credenciais R2 continuam exclusivas para backups.
5. Confira dados, migrations, auditoria e a pausa. O identificador do cluster deve
   diferir da origem; a ferramenta rejeita o banco de produção como destino.
6. Refaça os papéis pelo bootstrap, configure a aplicação atual e aplique migrations
   compatíveis antes de qualquer retomada. Preserve incertezas, compare Stories e
   ledger manualmente e aguarde a quarentena. Não reenvie publicações antigas.

Um dump diário admite perda de até 24 h de dados. A quarentena é obrigatória, mas
não prova entrega de Stories ausentes no snapshot. Essa limitação consta no ADR-016.
