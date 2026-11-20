# Relatório de entrega

Data: 20/11/2026. Repositório:
https://github.com/lincolncyber-01/breaking-bytes .

## Escopo entregue

Workspace de doze crates Rust, migrations, configuração, captura multicamada,
deduplicação, verificação N1/N2/N3, redação/segunda checagem, JPEG, agendamento,
publicação/reconciliação, tokens, correções, controles, métricas, alertas, backup,
restauração e ferramentas de lançamento. README e runbook explicam o uso;
[matriz](../requirements.md) relaciona cada requisito, implementação e prova.
Cada fase tem relatório próprio; decisões e desvios estão em docs/adr/.

**A entrega técnica não constitui aceite de produção.** Nenhuma conta real foi
publicada ou infraestrutura do proprietário implantada. As fases que exigem
corpus, credenciais e passagem real de tempo conservam esses portões abertos.

| Fase | Entrega técnica | Portão externo ainda aberto |
| --- | --- | --- |
| 1 — Fundação | Workspace, CI, config, migrations e health checks | Instalação no destino |
| 2 — Captura/deduplicação | Coletores, normalização, cluster e contratos/PG | 24 h reais e revisão semântica |
| 3 — Verificação/redação | Níveis, LLM, checagem e avaliação sintética | Chave/modelo e corpus real revisado |
| 4 — Imagens | Validação, template, pixels/Insta e benchmark release | Qualidade das imagens reais e desempenho da VPS |
| 5 — Publicação | R2/Graph, reconciliação e caos SIGKILL aprovados em contrato | Story real na conta de staging |
| 6 — Endurecimento | Métricas, alertas, papéis, backup/restore, scans e runbook | Alertas recebidos e backup/restore R2 reais |
| 7 — Canário | Release, rollout/rollback, observador e recibos | Dry-run 48 h, canário 72 h e promoção reais |

O [Story de exemplo](examples/story.jpg) é sintético, renderizado pelo mesmo
template. Os [benchmarks](benchmarks.md) contêm arquivos originais e reprodução:
CI release da fase 6 mediu render p95 53,957 ms, composição 16 MP 81,173 ms,
captura HTTP de contrato 4,930 ms/lote e cluster 3,031 ms. Pico do render foi
164.892 KiB; isso não representa memória do pipeline completo ou da VPS.

## Evidências e resultados

Os resultados da fase 6 estão vinculados ao commit 798d812: CI 44217875480,
segurança 44217875392 e fuzz 44217875610, todos aprovados. Foram 111 testes Rust
sem ignorados, cobertura de domínio 91,98% (não do workspace inteiro), três
parsers sob fuzz e imagens próprias sem HIGH/CRITICAL nem segredos. O teste de
caos usou SIGKILL real, Postgres real e Meta simulada; conservou um POST/efeito e
a intenção. Backup/restauração usaram dois clusters reais e S3 simulado.

A fase 7 passou localmente 111 testes Rust sem ignorados, 12 testes
operacionais, nove de backup/restauração e nove de ferramentas. As saídas reais
estão em evidence/phase-7-*. A toolchain foi comparada ao manifesto oficial
stable em 20/11/2026: Rust 1.100.0. Formatação/actionlint terminaram com código 0.
Clippy de todos os targets/recursos terminou sem avisos, com -D warnings.
Os checks completos da revisão entregue estão no
[PR #7](https://github.com/lincolncyber-01/breaking-bytes/pull/7), vinculados ao
SHA efetivamente executado. A integração de PostgreSQL não usa banco simulado.

Resultados de workflow manual de release/deploy não são alegados: só serão
obtidos após integrar a entrega e configurar o host. Testes de rollout verificam
contratos e recuperação. O [ensaio Compose complementar](phase-7-rollout.md)
acrescenta containers, processo, SQL e HTTP reais e regressão de shutdown;
não representa implantação na VPS do proprietário ou autenticação real no GHCR.

## Desvios e limites conhecidos

| Decisão | Limite / acompanhamento |
| --- | --- |
| hash_entities é o fallback permitido do blueprint | fastembed não implementado; paráfrases precisam de avaliação real, ADR-005 |
| Polling nos coletores suportados | WebSub e outros conectores sociais/licenciados não implementados, ADR-006 |
| Arte original como padrão e manifesto revisado | Detector automático de relevância/marca d'água não validado, ADR-009 |
| Prioridade conservadora | Impacto/alcance 0,5 e urgência 0 até calibração real, ADR-011 |
| Reconciliação preserva incerteza | A API não oferece transação distribuída; ambiguidades exigem prova humana, ADR-010 |
| Cota consultada em runtime | Documentação oficial contém limites conflitantes de 100/50, ADR-002 |
| Executável GNU/glibc | Não é estaticamente linkado; imagem mínima validada em amd64, ADR-018 |
| SBOM por artefatos efetivamente compilados | Stable Cargo sobreaproxima dependências opcionais; política recusa backend vulnerável se compilado, ADR-018 |
| Três avisos de manutenção temporários | Exceções exatas até 17/12/2026; sem exceção de CVE, ADR-017 |
| Aceite vinculado a imagem/configuração | Mudanças de versão/identidade invalidam o vínculo; recibo local não prova revisão independente, ADR-019 |
| Métricas e limites de recursos | SLO mensal, rede comercial e carga completa na VPS ainda não demonstrados |
| Scans de imagens próprias | Não equivalem a scan do host nem de toda imagem de fornecedor; atualizações continuam necessárias |

## Ações manuais em ordem

1. Revisar o PR completo contra main; configurar revisores, proteção do branch
   e environments release/production. Integrar a revisão aprovada e conferir
   os checks do SHA em main.
2. Criar/configurar o app Meta, fluxo de login e App Review; conceder permissões
   oficiais e vincular conta profissional/Página quando aplicável. Providenciar
   conta separada de staging e confirmar a cota real.
3. Obter tokens e metadados/recibo do fluxo escolhido, configurar inspeção ou
   renovação e testar vencimento/rotação. Fornecer segredos só pelo ambiente.
4. Criar buckets R2 de imagens e backup, staging separado e credenciais de
   menor privilégio. Validar URL temporária acessível pela Meta e retenção.
5. Configurar chave/modelo LLM, fontes/licenças/robots, contato SEC e origin_group.
   Avaliar corpus real e revisar fidelidade, deduplicação e qualidade visual.
6. Preparar VPS/runner dedicado, firewall/TLS, armazenamento e papéis do banco;
   executar release versionado, conservar digests/SBOM/scans e fazer bootstrap.
   Não apagar volumes ou histórico de intenção durante atualização.
7. Configurar Telegram, Grafana/Prometheus e timers; comprovar recebimento de
   alerta, backup R2 e restauração em cluster isolado, incluindo reboot e
   reinjeção de ambiente pelo provedor de segredos.
8. Executar staging real na conta de teste; validar pausa, preflight, token,
   render, publicação e reconciliação sem duplicação.
9. Executar coleta real de 24 h e dry-run real de 48 h, medir p95/pico sob carga,
   revisar conteúdo e emitir o recibo com SQL e evidências íntegras.
10. Executar canário real de 72 h a 10/dia; só depois aprovar promoções para
    20/40/80, observando 72 h em cada patamar. Acompanhar SLO mensal de 99,5%.

Comandos completos, campos de revisão, expiração e recuperação estão em
[launch.md](../launch.md). Não há credenciais ou recibo aprovado inventados no
repositório. O pipeline permanece em dry-run por padrão.
