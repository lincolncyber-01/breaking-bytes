# ADR-008: contrato atual da Messages API

Estado: conferido na documentação oficial; contrato exercitado com servidor simulado.

Fontes oficiais consultadas nesta fase:

- https://platform.claude.com/docs/en/api/messages/create
- https://platform.claude.com/docs/en/build-with-claude/structured-outputs
- https://platform.claude.com/docs/en/build-with-claude/effort
- https://platform.claude.com/docs/en/models/haiku-5-5/overview

Mantemos `claude-haiku-5-5`. O endpoint é POST
`https://api.anthropic.com/v1/messages`, com `x-api-key` e
`anthropic-version: 2026-10-21`. A interface atual usa
`output_config.format` com `type=json_schema`; não usamos o campo antigo
`output_format`. Haiku 5.5 não aceita os antigos controles de amostragem.
Usamos esforço medium e thinking desativado. A compatibilidade do modelo com a
conta, cota e cobrança exige chave real; a consulta documental não demonstra isso.

O schema enviado usa o subconjunto estrutural aceito pela API. Restrições de
comprimento e quantidade são aplicadas em Rust. `jsonschema` 0.61.2 compila sem
resolução externa de schemas; respostas não podem introduzir referências remotas.

O cliente só aceita conclusão `end_turn` e um bloco JSON de texto. Recusas,
truncamento, ações de ferramentas e corpos inválidos bloqueiam o fluxo. Limites:
64 KiB de dados por chamada, 128 KiB de resposta HTTP, 4096 tokens e timeout
configurável de 1–60 segundos. 429 e 5xx transitórios voltam à fila com backoff;
Retry-After de até 24 horas é preservado. Erros não contêm corpo ou credenciais.

O transporte da aplicação é compartilhado, sem redirecionamento automático.
O destino LLM é fixo; URLs jornalísticas continuam com robots, DNS público e
limites do coletor. A avaliação isolada usa o mesmo destino fixo e não publica.
Não removemos proxies da Cloud para contornar suas restrições de rede.
