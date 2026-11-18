# Benchmarks e limites de interpretação

Medições em 18/11/2026, Rust 1.100.0. Os JSONs originais ficam em `evidence/`.
Não se subtraíram amostras lentas nem se simulou latência de provedores externos.

| Medição | Ambiente / amostras | Resultado |
| --- | --- | --- |
| Captura HTTP local, robots, parsing e normalização de 200 itens | CI release, 50 lotes | p50 4,483 ms; p95 4,642 ms; máximo 6,231 ms; RSS do teste 11.720 KiB |
| Ingestão e agrupamento de 200 matérias equivalentes | CI release, Postgres real | rajada 784,561 ms; cluster p95 2,427 ms; máximo 3,686 ms |
| Arte original 1080×1920 e JPEG90 | CI release, Xeon Platinum 8370C, 50 renders | p50 56,711 ms; p95 58,266 ms; máximo quente 59,185 ms; primeiro render 69,657 ms |
| Composição com imagem de 16 MP | Mesmo processo release | p95 86,409 ms; pico RSS do processo 164.892 KiB, cerca de 161 MiB |
| Binário real ocioso após readiness + 1 segundo | Ambiente local, debug | RSS 38.808 KiB |
| Captura de 200 itens | Ambiente local, debug, 50 lotes | p95 82,134 ms |
| Clusterização de 200 matérias | Ambiente local, debug, Postgres real | p95 5,223 ms; rajada 1,480 s |

Fonte CI: execução 44203846339, commit 5a014bf, job 134464595506; o JSON do
render veio do artefato 14383247571 e os JSONs de captura/cluster do stdout real.
Os caminhos de saída desses dois testes e da memória ociosa foram corrigidos
para absolutos no workflow, pois Cargo executa testes no diretório do crate.
A nova execução deve conservar os três arquivos no artefato do job.

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
