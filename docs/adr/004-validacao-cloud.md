# ADR-004: validação em ambiente Cloud com disco limitado

Data: 16/11/2026. Estado: aceito apenas para o ambiente de testes desta sessão.

O ambiente tem volume de 8 GB e Docker rootless com driver VFS. A imagem
pgvector/pgvector:0.8.2-pg16 tem 16 camadas; as cópias integrais de camadas
esgotaram o volume durante a criação de containers e o build da imagem Rust.
As primeiras falhas foram de infraestrutura, e estão registradas como tal.

Para testar o banco, salvamos a imagem oficial do digest fixado no Compose e
geramos uma imagem de uma camada, aplicando os whiteouts e preservando metadados,
conteúdo dos arquivos, variáveis e entrypoint. Ela recebeu o nome local
breaking-bytes/pgvector-fixture:0.8.2-pg16, diferente da imagem oficial.
Nenhum binário de Postgres ou pgvector foi substituído. O teste consulta a
extensão instalada e exige pgvector 0.8.2.

STORIES_TEST_POSTGRES_IMAGE permite selecionar essa imagem somente nos testes.
O padrão dos testes e do CI permanece pgvector/pgvector:0.8.2-pg16.
Executamos os testes com um processo por vez para controlar o uso do disco.
Não há skip ou substituição do Postgres por um mock ou por SQLite.

O build Docker completo falhou ao registrar a camada da imagem de compilação
por falta de espaço. Sua validação permanece pendente; não foi convertida em
sucesso com um build de outra imagem. O build release nativo é uma verificação
separada. Uma execução em CI com armazenamento suficiente deverá validar o Dockerfile.
