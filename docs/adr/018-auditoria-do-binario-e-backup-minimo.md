# ADR 018 — Auditoria do binário entregue e runtime mínimo do backup

Data: 2026-11-18. Estado: aceito; gates automatizados obrigatórios no CI.

## Evidência e decisão

A auditoria integral de Cargo.lock encontrou RUSTSEC-2026-0459 / CVE-2026-119375
em libsqlite3-sys 0.37.0. O lock do SQLx 0.9 também resolve dependências opcionais
não ativadas. `cargo tree --all-features --invert libsqlite3-sys` não encontra
qualquer caminho no produto exclusivamente Postgres. A correção >=0.39 é
incompatível com a faixa opcional do SQLx atual; não há exceção para esse CVE.

O executável final passa a conter o inventário produzido pelo cargo-auditable
0.7.7. O CI extrai esse executável da imagem, valida a seção comprimida, exige
uma raiz stories-app e versões presentes no lock, recusa SQLite/MySQL e executa
`cargo audit --deny warnings bin`. Cargo-deny continua verificando o workspace.
Ausência ou divergência do inventário falha o gate. Isso audita o código entregue
sem declarar seguro um componente vulnerável que não é usado. Uma futura
ativação de SQLite exige atualizar SQLx/dependências e rever esta decisão.

A varredura da imagem operacional anterior encontrou vulnerabilidades altas e
críticas em ferramentas e bibliotecas herdadas, inclusive gosu não utilizado.
O backup agora usa Python 3.15.0 e clientes PostgreSQL 16.15 compilados da fonte
oficial com SHA-256 verificado. A imagem final distroless contém apenas Python,
AWS CLI, clientes e suas bibliotecas dinâmicas. Não contém servidor, apt, gosu,
Perl, LDAP ou Kerberos. Mantém TLS via OpenSSL e os metadados dpkg/copyright das
bibliotecas copiadas para que o scanner continue identificando versões reais.
Os módulos SQLite e Tk da biblioteca padrão não participam do utilitário e
são removidos; o runtime não inclui banco SQLite nem interface gráfica.
Não se ignora nenhuma vulnerabilidade alta/crítica encontrada na imagem final.

## Verificação e limites

Testes do validador rejeitam inventário divergente, backends proibidos e dados
comprimidos truncados ou excessivos. O CI constrói ambas as imagens, executa
os clientes e exige Trivy sem HIGH/CRITICAL nem segredos. O contrato de backup
usa dois clusters reais e armazenamento HTTP simulado. Compatibilidade com R2
real, restauração do volume real e cron na VPS continuam exigindo credenciais.

Fontes: https://rustsec.org/advisories/RUSTSEC-2026-0459.html ;
https://github.com/rust-secure-code/cargo-auditable ;
https://ftp.postgresql.org/pub/source/v16.15/ ;
https://github.com/GoogleContainerTools/distroless .


## Compatibilidade de distribuição

O blueprint chama o executável de estático. O artefato GNU validado usa glibc e
libgcc dinamicamente; ldd e a imagem sem shell confirmam isso. A alternativa
adotada conserva a stack exigida (especialmente ring, fontes e SDK AWS) e mantém
apenas essas bibliotecas na imagem. Não se declara link estático nem suporte a
musl sem build, snapshots e benchmarks próprios. O build release continua com
LTO fat, um codegen unit, panic abort e strip, conforme o blueprint.
