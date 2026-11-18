# Benchmarks e limites de interpretação

Medições em 18/11/2026, Rust 1.100.0. Os JSONs originais ficam em `evidence/`.
Não se subtraíram amostras lentas nem se simulou latência de provedores externos.

| Medição | Ambiente / amostras | Resultado |
| --- | --- | --- |
| Captura HTTP local, robots, parsing e normalização de 200 itens | CI release, 50 lotes | p50 4,834 ms; p95 4,930 ms; máximo 7,485 ms; RSS do teste 11.612 KiB |
| Ingestão e agrupamento de 200 matérias equivalentes | CI release, Postgres real | rajada 948,334 ms; cluster p95 3,031 ms; máximo 10,629 ms |
| Arte original 1080×1920 e JPEG90 | CI release, AMD EPYC 7763, 50 renders | p50 52,633 ms; p95 53,957 ms; máximo quente 55,860 ms; primeiro render 64,723 ms |
| Composição com imagem de 16 MP | Mesmo processo release | p95 81,173 ms; pico RSS do processo 164.892 KiB, cerca de 161 MiB |
| Binário real ocioso após readiness + 1 segundo | Ambiente local, debug | RSS 38.808 KiB |
| Captura de 200 itens | Ambiente local, debug, 50 lotes | p95 82,134 ms |
| Clusterização de 200 matérias | Ambiente local, debug, Postgres real | p95 5,223 ms; rajada 1,480 s |

Fonte final CI: execução 44217875480, commit 798d812, job 134512138537,
artefato 14389143506. Os três JSONs foram efetivamente baixados do artefato e
ficam em `phase-6-final-release-*`. As medições anteriores, no Xeon Platinum
8370C (execução 44203846339), permanecem como histórico em `phase-6-release-*`:
render p95 58,266/86,409 ms, captura p95 4,642 ms e cluster p95 2,427 ms.
Os caminhos de saída dos testes agora são absolutos ao workspace; antes, dois
arquivos eram gravados no diretório do crate e ficaram disponíveis só no stdout.

Os p95 de render e agrupamento atendem às metas de 150 ms e 50 ms no ambiente
medido. Os resultados não demonstram prazo absoluto: a fase 4 registrou máximo
de 413,8 ms. Agendamento de CPU e diferenças da VPS podem mudar os percentis.

Detecção ponta a ponta inclui intervalo de polling, propagação da fonte e rede;
o benchmark de contrato mede somente a captura disponível. LLM, upload R2 e Meta
não foram medidos com credenciais reais. Memória ociosa não representa pico do
pipeline completo. Não se demonstrou o orçamento de uma VPS de 1 vCPU/1 GB sob
carga: Postgres, observabilidade e backup também consomem memória. Inicie com
2 GB ou dimensione após medir a composição e a concorrência reais.

Reprodução:

```sh
cargo bench --package stories-imaging --bench story --locked -- --sample-size 20
cargo run --release --locked --package stories-imaging --example benchmark -- artifacts/benchmark.json
STORIES_DETECTION_BENCHMARK="$PWD/artifacts/detection.json" cargo test --release --package stories-collectors --locked simulated_detection_benchmark -- --nocapture
STORIES_CLUSTER_BENCHMARK="$PWD/artifacts/clustering.json" cargo test --release --package stories-store --test collection --locked atomic_ingest_concurrent_clustering_updates_and_burst -- --nocapture
```
