# Compilar e testar no CI Linux x86_64

Este guia prepara uma compilação nativa em Linux x86_64 e gera o executável de release. Não há workflow de CI ativo neste repositório; copie os passos para o seu runner.

## Ambiente

- Runner Linux x86_64, de preferência Ubuntu 24.04.
- Rust stable 1.88 ou superior, conforme `rust-version` no workspace.
- Pacotes de desenvolvimento usados pelo `eframe` no Linux:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev
```

A lista de pacotes vem do guia oficial do [`eframe`](https://github.com/emilk/egui/tree/master/crates/eframe#readme). O binário de interface é nativo e pode depender de bibliotecas gráficas disponíveis no sistema onde for executado.

## Passos do job

O repositório já contém `Cargo.lock`. Na raiz clonada, execute:

```sh
rustc --version
cargo fmt --all -- --check
cargo test --workspace --locked
cargo build --release --locked -p motion-editor
```

`--locked` impede que o job altere as versões de dependências usadas. Se uma mudança nos manifests exigir atualizar o lockfile, regenere-o deliberadamente, revise e versione a alteração antes de gerar o binário.

O executável esperado é:

```text
target/release/motion-editor
```

Para anexá-lo como artefato, compacte ao menos o binário e o lockfile:

```sh
mkdir -p dist
cp target/release/motion-editor dist/
cp Cargo.lock dist/
tar -czf motion-editor-linux-x86_64.tar.gz -C dist .
```

Anexe `motion-editor-linux-x86_64.tar.gz` ao job e envie-o de volta aqui. Inclua também o log de `cargo test` e `cargo build`; se o build falhar, envie o primeiro erro completo e o sistema/runner usado. Inclua o `Cargo.lock` do commit usado para gerar o binário.

## Limites da compilação

Um build bem-sucedido valida que o código compila; ele não adiciona codecs. Esta versão ainda não decodifica vídeo/áudio nem exporta MP4/H.264/AAC. Como o aplicativo tem interface gráfica, o runner só precisa compilar; para abrir o binário, use uma sessão Linux gráfica com as bibliotecas de runtime correspondentes.
