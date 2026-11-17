# ADR-003: modelo editorial configurável

Data: 16/11/2026. Estado: confirmado documentalmente; cliente na fase 3.

A documentação oficial identifica `claude-haiku-5-5`, como no blueprint:
https://platform.claude.com/docs/en/models/haiku-5-5/overview

Mantemos esse identificador no exemplo. Disponibilidade para a chave, permissões,
limites, custo e latência efetiva serão verificados quando houver credenciais.
O modelo pode ser sobrescrito por STORIES__LLM__MODEL, sem alterar o código.
Não há chamada a LLM na fundação e nenhum resultado editorial foi validado.
