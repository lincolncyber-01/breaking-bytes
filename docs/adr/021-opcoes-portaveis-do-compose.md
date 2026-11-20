# ADR-021: usar a opção portátil de execução sem TTY

Data: 20/11/2026. Estado: corrigido; validação final no CI.

O ensaio local passou em Compose 2.40.3 e 5.8.0, incluindo autenticação SCRAM,
senha inválida recusada e stdin fechado. O runner do GitHub recusou a primeira
pausa com `unknown flag: --no-tty`, antes de executar psql. O diagnóstico foi
conservado em artefato com credenciais removidas, sem imprimir ambiente ou
senha. Não se atribui essa falha à autenticação ou ao schema do banco.

A opção longa aparece como --no-TTY em versões anteriores e --no-tty nas
versões locais verificadas. Adotamos -T no helper de produção e no cliente HTTP
interno do ensaio, preservando o comportamento sem depender dessa grafia.
O ensaio conserva também a versão real do Compose no relatório e executa com
stdin fechado quando não transmite SQL pelo pipe. Para consultas -f -, o pipe
continua sendo encaminhado normalmente.

O fixture PostgreSQL local pode ser forçado a SCRAM; o CI mantém a configuração
da imagem PostgreSQL de produção. A checagem negativa de senha demonstra que o
sucesso local não depende de trust. Não se mudou o fluxo de migração, papel de
runtime, auditoria ou autenticação de produção para contornar o erro.

Fontes oficiais: https://docs.docker.com/reference/cli/docker/compose/exec/ ;
https://github.com/docker/compose/blob/v2.40.3/cmd/compose/exec.go ;
https://github.com/docker/compose/blob/v5.8.0/cmd/compose/exec.go .

Evidência da falha: execução 44801306890, artefato compose-rollout 14660549090.
