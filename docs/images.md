# Operação das imagens

O padrão usa arte original e funciona sem credencial de imagem. O manifesto
`config/images.json` contém uma lista vazia válida; não há imagens externas
aprovadas implicitamente. O caminho `imaging.manifest` é relativo ao diretório
do TOML carregado. `imaging.artifacts_dir` é relativo ao diretório de execução,
ou pode ser absoluto. Em Docker, o volume fica em `/app/artifacts`.

Para habilitar imagens externas, cadastre os hosts exatos em allowed_domains e
adicione arquivos revisados ao manifesto. Não use curingas, URLs assinadas,
credenciais, query strings ou redirects para outro domínio. O processamento
respeita robots e não deve contornar a rede gerenciada da Cloud.

Cada objeto do manifesto exige os campos:

| Campo | Conteúdo exigido |
| --- | --- |
| url | URL HTTPS estável do arquivo aprovado |
| role | press_kit, primary, og ou library, nessa ordem de preferência |
| article_url | URL da matéria associada, ou null quando a relação for por entidade |
| entities | Nomes que precisam corresponder às entidades do texto aprovado |
| credit | Crédito visível, em uma linha, com até 100 caracteres |
| license | owned, cc0, cc-by ou press-permission |
| rights_url | Página HTTPS que documenta a licença ou autorização |
| blake3 | Hash hexadecimal de 64 caracteres dos bytes exatos |
| review.reviewer | Responsável pela revisão |
| review.reviewed_at | Data/hora RFC3339, não futura e com até 180 dias |
| review.watermark_free | true somente após inspeção de marcas d'água |
| review.relevant | true somente quando houver relação com a notícia |
| review.generic | false somente quando não for imagem genérica inadequada |

A associação precisa corresponder à matéria ou a todas as entidades indicadas.
Para role og, matéria e og:image precisam corresponder simultaneamente. Guarde
a evidência da autorização e inclua os termos de atribuição no crédito. O hash
deve ser calculado sobre o arquivo original, sem convertê-lo antes da revisão.
Os testes de integridade não determinam se a autorização é juridicamente válida.

Na indisponibilidade, mudança de hash, MIME falso, excesso de pixels, falha de
licença/revisão ou crédito incompatível, o worker usa arte original. Não publique
um arquivo mudando apenas o hash para fazer o controle passar: revise de novo.
Após alterar o manifesto, reinicie a aplicação para carregar a nova configuração.

Para inspecionar o exemplo offline:

```sh
cargo run --locked --bin stories-tech -- --render-example artifacts/example.jpg
```

O exemplo é sintético, sem chamada ao LLM, sem banco e sem publicação. O JSON ao
lado do JPEG contém as medidas de layout. Em distribuição de binário, inclua
também `assets/fonts/LICENSE.txt`; a imagem Docker conserva essa licença em
`/app/licenses/Inter-OFL.txt`.

As referências visuais ficam em crates/imaging/tests/golden. Atualizações são
explícitas, pelo exemplo reference, e devem ser inspecionadas antes do commit.
Os testes comparam pixels do JPEG decodificado e não reescrevem as referências.

```sh
cargo run --locked --package stories-imaging --example reference -- crates/imaging/tests/golden
cargo bench --locked --package stories-imaging --bench story -- --warm-up-time 1 --measurement-time 3 --sample-size 20 --noplot
cargo run --release --locked --package stories-imaging --example benchmark -- artifacts/benchmark.json
```

O benchmark mede CPU e RSS do renderizador em Linux, com arte original e imagem
sintética de 16 MP já decodificada. Não representa latência de detecção, rede,
LLM ou memória de toda a aplicação sob carga. Esses testes operacionais continuam
nos marcos seguintes. O comando retorna erro quando o p95 atinge 150 ms ou o RSS
de pico atinge 300 MiB.
