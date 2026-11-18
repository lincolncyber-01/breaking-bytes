# ADR 018 — Auditoria do binário entregue e runtime mínimo do backup

Data: 2026-11-18. Estado: aceito; gates automatizados obrigatórios no CI.

## Evidência e decisão

A auditoria integral de Cargo.lock encontrou RUSTSEC-2026-0459 / CVE-2026-119375
em libsqlite3-sys 0.37.0. O lock do SQLx 0.9 também resolve dependências opcionais
não ativadas. `cargo tree --all-features --invert libsqlite3-sys` não encontra
qualquer caminho no produto exclusivamente Postgres. A correção >=0.39 é
incompatível com a faixa opcional do SQLx atual; não há exceção para esse CVE.

O executável final contém inventário no formato interoperável .dep-v0, descrito
pelo cargo-auditable. O CI extrai esse executável da imagem, valida a seção comprimida, exige
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


## Correção do inventário no Rust estável

A execução 44210767519 recusou o inventário do cargo-auditable 0.7.7 por incluir
SQLite não compilado. A reprodução local chamou o wrapper instalado apontando
para os metadados do app e confirmou a inclusão. A documentação e o código do
próprio fornecedor reconhecem essa sobreaproximação de cargo metadata nos
formatos 0/1; o precursor preciso requer Cargo nightly, incompatível com a
exigência de toolchain estável desta entrega.

A alternativa implementada usa `cargo build --message-format=json` no build
release real. `compiler-artifact` identifica todos os pacotes efetivamente
compilados, incluindo build dependencies e proc macros; `build-finished` precisa
ser bem-sucedido e o artefato raiz precisa ser o executável stories-tech esperado.
O exportador cruza esses IDs com cargo metadata, conserva o grafo alcançável de
dependências normais/build, classifica proc macros como build e exige que nenhum
pacote compilado fique sem caminho na raiz. Não há filtro por nome, advisory ou
resultado de scanner. A única redução da sobreaproximação vem da prova de
compilação do próprio Cargo.

O JSON segue o formato 1 público e é comprimido com zlib. GNU objcopy acrescenta
a seção somente leitura após o link/strip; não altera instruções do programa.
A seção é extraída e validada no builder e novamente a partir do executável da
imagem. Cargo-audit e Trivy continuam lendo o inventário real entregue. O gate
que rejeita SQLite/MySQL permanece independente do exportador; nenhum CVE é
ignorado. A procedência é essa exportação de metadados mais compiler-artifacts,
não um precursor nativo de Cargo nightly nem uma declaração de código removido
por otimização/LTO.

O teste integra duas compilações Cargo reais em workspace descartável: um backend
opcional ausente fica fora do inventário; ao ativá-lo, o inventário o conserva e
a política bloqueia a entrega. Outro teste recusa compilação sem conclusão e
artefato sem ligação ao grafo. A cadeia de build é confiável; o SBOM documenta
componentes, sem pretensão de detectar compilador ou dependência maliciosos.

As bibliotecas do runtime operacional preservam seus SONAMEs no diretório
canônico /usr/lib, sem substituir o symlink /lib da base. O bootloader AWS usa
bibliotecas de sistema; módulos privados do AWS CLI são inspecionados no seu
ambiente privado. O build final executa Python/SSL/fuso e todos os clientes.
O CI 44210767519 já validou os clientes 16.15, SSL 3.5.7 e restauração real usando
a imagem entregue; os novos gates do inventário exigem nova execução.

## Minimização final dos clientes operacionais

O scan 44213434400 aprovou o inventário de 389 crates e a imagem da aplicação,
sem vulnerabilidades HIGH/CRITICAL ou segredos. A imagem de backup ainda continha
pip 26.2.1 e seu código vendorizado, além de módulos interativos/UUID opcionais.
O relatório marcou certifi, msgpack, setuptools e urllib3 do inventário pip;
libncursesw6/libtinfo6 e libuuid1 receberam avisos sem correção no fornecedor.
O resumo sanitizado conserva esses achados. Nenhum recebeu exceção.

Pip, ensurepip e seu código vendorizado são removidos integralmente do runtime;
não se remove somente o inventário para ocultar código. O utilitário operacional
usa apenas a biblioteca padrão e os clientes já construídos. Readline, curses e
o acelerador C opcional de UUID são removidos do Python operacional. Na
empacotada CLI AWS, readline/_uuid e suas bibliotecas privadas também são
removidos. Não se altera o transporte TLS nem o SDK S3. UUID4 mantém a geração
pela fonte criptográfica os.urandom da biblioteca padrão. A funcionalidade
interativa da CLI não integra o contrato operacional desta imagem.

A distribuição oficial 2.39.6 foi inspecionada e ensaiada localmente sem esses
módulos/bibliotecas. Um novo contrato executa a CLI real contra servidor S3
local, confere independentemente SigV4 e os quatro métodos PUT/GET/list/DELETE,
bytes e modelo XML. No Docker o contrato roda com --network=none e fixture por
bind temporário; nenhum servidor remoto ou credencial real participa, e o script
não é incorporado à imagem final. O contrato também confirma ausência dos
módulos/pip e UUID4 válido. Todos os vínculos dinâmicos restantes continuam
inventariados com metadados dpkg, e Trivy precisa aprovar a nova imagem.


Resultado final: execução 44217875392, commit 798d812, aprovou ambas as imagens
com zero HIGH/CRITICAL e zero segredos, sem exceção de CVE. Os contratos da imagem
minimizada, a auditoria de 389 crates, o render offline e os demais checks também
passaram. A decisão está verificada automaticamente; serviços reais seguem o
aceite operacional documentado.
