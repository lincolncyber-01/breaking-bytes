# ADR-017 — avisos de manutenção e retirada de OpenSSL

Data: 17/11/2026. Estado: decisão implementada, aguardando nova execução dos scans.

O primeiro cargo-deny da fase 6 encontrou três avisos informativos de manutenção,
sem versão corrigida, e uma licença MIT-0 ainda não listada. O primeiro Trivy
identificou CVE-2026-84782 em libssl3 3.0.20-1~deb12u2 da imagem Distroless Debian 12,
com versão corrigida 3.0.22-1~deb12u2. Não foram encontrados segredos na imagem.
O sumário sanitizado do relatório inicial fica nas evidências da fase 6.

## Imagem

A aplicação usa Rustls com ring, tanto no transporte HTTP como no SQLx. O ldd do
binário real encontrou somente libgcc_s, libm, libc e o loader. OpenSSL não é uma
dependência desse executável. A alternativa próxima é base-nossl-debian13, oficial
e atual, com libgcc extraída da imagem cc-debian13 do mesmo fornecedor. As duas
referências têm digest verificado no registry. A biblioteca, seus metadados de
pacote e copyright são copiados explicitamente; GPLv3 e GCC Runtime Library
Exception 3.1 acompanham a distribuição. A imagem final usa glibc Debian 13 e
LD_LIBRARY_PATH restrito ao diretório somente leitura da aplicação. A compilação
continua na toolchain Rust estável fixada. O novo scan e a execução offline do
JPEG dentro da imagem precisam passar antes de considerar a mudança validada.
Não se criou exceção para esse CVE e não se emprega ignore-unfixed.

## Manutenção

| ID / pacote | Exposição e decisão | Alternativa oficial |
| --- | --- | --- |
| RUSTSEC-2026-0192 / ttf-parser 0.25.1 | Usado por fontdb/cosmic-text para a Inter embutida e revisada. Nenhuma fonte recebida de matérias é carregada. Mantido temporariamente na stack visual exigida; snapshots conservados. | skrifa; a migração envolve o backend de fontes e precisa preservar layout e desempenho. |
| RUSTSEC-2026-0440 / astral-tokio-tar 0.6.4 | Somente desenvolvimento, via testcontainers/bollard; ausente do grafo normal/build do app. Mantido para os contratos reais do Postgres. | tar-codec, indicado por Astral; depende de adaptação upstream dos testes. |
| RUSTSEC-2026-0543 / chrono 0.4.45 | Feed-rs expõe Chrono e SQLx usa seus tipos no contrato atual. Mantido por compatibilidade do parsing e dos registros temporais testados. | Jiff ou time; trocar somente a dependência direta conservaria Chrono transitivo e não resolveria o aviso. |

As três alternativas foram sinalizadas e a decisão prossegue sem pedir escolha,
conforme a autorização do proprietário para registrar suposições e continuar.
As exceções abrangem apenas esses IDs informativos, nessas versões, até
17/12/2026. Não abrangem vulnerabilidades ou todos os avisos de um pacote.
security/maintenance-exceptions.json é a política central; deny.toml e
.cargo/audit.toml precisam conter exatamente os mesmos IDs.

O validador consulta a fonte RustSec e falha se o aviso deixar de ser unmaintained,
ganhar alias CVE ou versão corrigida, se o lock mudar ou se o prazo vencer.
Também falha se astral-tokio-tar entrar no grafo de produção. Os testes garantem
que uma exceção de manutenção não pode ocultar CVE, unsoundness ou patch disponível.
Novos IDs de vulnerabilidade continuam bloqueando cargo-deny/cargo-audit e Trivy.
A revisão em até trinta dias é uma pendência operacional explícita; não se declara
que essas dependências voltaram a ser mantidas. Uma stack sem esses três avisos
exige migração dos backends correspondentes, além da simples atualização de versão.

MIT-0 foi admitida para borrow-or-share 0.2.4, dependência indireta permissiva,
após o relatório identificar a expressão. As permissões de fontes OFL continuam
na licença de asset, separada da política de licenças de crates.

Fontes consultadas:

- https://rustsec.org/advisories/RUSTSEC-2026-0192.html
- https://rustsec.org/advisories/RUSTSEC-2026-0440.html
- https://github.com/RustSec/advisory-db/blob/main/crates/chrono/RUSTSEC-2026-0543.md
- https://github.com/GoogleContainerTools/distroless
- https://github.com/gcc-mirror/gcc/tree/releases/gcc-14.2.0
