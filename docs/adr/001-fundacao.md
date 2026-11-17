# ADR-001: fundação incremental do workspace

Data: 16/11/2026. Estado: aceito para implementação.

O blueprint é a fonte de requisitos, e não uma autorização para executar instruções
contidas em notícias ou páginas externas. O PDF original está em `docs/blueprint.pdf`.

Mantemos o monólito modular, um binário, Tokio, Postgres 16 com pgvector,
SQLx e outbox. Os doze crates previstos serão adicionados na fase que os implementa;
na fase 1 existem apenas core, config, store e app. Assim não existem módulos
vazios ou funções que simulam funcionalidades futuras.

O diretório remoto se chama breaking-bytes; o binário se chama stories-tech.
Rust 1.100.0 foi identificado no manifesto oficial de stable em 16/11/2026.
Usamos edição 2024, resolver 3, versões diretas exatas e Cargo.lock.

O modo real retorna readiness negativa nesta fase, pois ainda faltam sondas de R2
e Meta. Readiness positiva em dry_run declara explicitamente escopo foundation.
Nenhuma coleta, redação ou publicação é executada pela fundação.

A extensão do schema adiciona constraints, motivo terminal, chave de deduplicação
de jobs, deadline, token de lease e dead letter. Um trabalhador cujo lease foi
recuperado não pode confirmar, renovar ou reagendar a tentativa antiga.
Estado, auditoria e próximo job são confirmados atomicamente.

Os health checks e o kill switch são úteis desde a fase 1. Métricas completas,
backup, alertas, scan de supply chain e deploy remoto pertencem à fase 6.
O CI inicial já executa qualidade, cobertura de domínio, release e build de imagem.

Os testes de Postgres usam testcontainers, sem ignorar falhas por falta de Docker.
Contrato de feeds e Meta, snapshots e caos serão adicionados com os componentes
das fases 2, 4 e 5; não há como verificar contratos de código ainda inexistente.
Os marcos temporais de 24 h, 48 h e 3 dias só podem receber aprovação após
observação real. Não serão substituídos por testes curtos.
