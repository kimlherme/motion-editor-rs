# Arquitetura e escopo da primeira versão

## Alvo

- Workspace Rust edition 2024, MSRV 1.88 ou superior e alvo inicial Linux x86_64.
- Modelo e formato de projeto próprios. O formato externo é JSON versionado; `.mproj` e `motion-editor.project` são nomes provisórios.
- Não há compatibilidade com projetos do Alight Motion. O aplicativo não executa o APK e não contém código, assets, marca ou arquivos proprietários desse produto.
- FilmCraft pode servir de referência arquitetural, mas não há dependência de código FilmCraft nesta versão. Qualquer reutilização futura depende de revisão de licença e proveniência.

## Modelo de tempo e composição

- `Tick(i64)` usa `TICKS_PER_SECOND = 254_016_000_000`. Taxas de frame e bases de tempo são racionais. Conversões e produtos devem usar aritmética verificada.
- O projeto contém canvas, taxa de frames, assets, tracks e clips com IDs estáveis. O MVP usa uma sequência direta, sem composições aninhadas.
- Tracks visuais compõem de baixo para cima. Clips e sequences usam intervalos semiabertos `[start, end)`.
- A posição do transform é relativa ao centro do canvas, com Y positivo para baixo. O anchor é normalizado, `[0.5, 0.5]` representa o centro da imagem, a escala é por eixo, a rotação usa graus e a opacidade fica entre 0 e 1.
- Keyframes guardam ticks relativos à origem da fonte. A avaliação converte tempo da sequência em tempo da fonte. Stills usam um relógio sintético. Interpolações disponíveis: Linear, EaseIn, EaseOut e EaseInOut (smoothstep).
- Retiming, máscaras, efeitos e blend modes ficam fora do escopo atual.

## Mídia, preview e export

- Metadados de mídia preservam PTS e `time_base` racionais por stream. O tempo do projeto não substitui o relógio original do decoder.
- O compositor CPU define a referência para ordem de layers, avaliação, transform e alpha no preview e no export.
- Um frame antigo não deve ser apresentado como se correspondesse ao playhead atual. Ausência de frame e erros de decode devem permanecer explícitos.
- Export recebe um snapshot imutável e um encoder por trait, sem acoplar o modelo do projeto a um codec.
- A implementação atual não inclui decoder de vídeo/áudio nem encoder MP4/H.264/AAC. A exportação falha explicitamente quando nenhum encoder é fornecido.

## Crates e fronteiras

| Crate | Responsabilidade |
|---|---|
| `motion-core` | Tipos do domínio, tempo, validação e persistência versionada. |
| `motion-animation` | Avaliação pura de keyframes e transforms. |
| `motion-media` | Importação/probe de PNG/JPEG e contratos para frames e timestamps de mídia. |
| `motion-render` | Composição CPU a partir do projeto, avaliador e provider de frames. |
| `motion-export` | Snapshot, fila limitada, cancelamento, progresso, publicação atômica e adapter de encoder. |
| `motion-engine` | Sessão e comandos do editor. |
| `motion-ui` | Interface nativa egui que conecta os módulos. |
| `apps/editor` | Binário executável `motion-editor`. |

## Contratos entre módulos

```text
motion_core::time::{Tick, FrameRate, TICKS_PER_SECOND}
motion_core::project::{Project, Canvas, MediaAsset, MediaKind, Track, TrackKind,
                       Clip, Transform, Vec2, Keyframe<T>, Interpolation,
                       TransformKeyframes}
motion_core::persistence::{save_atomic(path, project), load(path)}
motion_animation::evaluate_transform(clip, source_time)
motion_media::{FrameProvider, RgbaFrame, MediaError}
motion_render::render_frame(project, sequence_time, frame_provider)
motion_engine::EditorSession
motion_export::{ExportRequest, VideoEncoder, run_export(snapshot, request, provider, encoder)}
motion_ui::run()
```

O provider de frames recebe um asset e o tempo da fonte, e retorna um frame RGBA8 imutável junto ao timestamp original (`pts` e `time_base`). O render resolve `source_in + (sequence_time - timeline_start)` e seleciona clips ativos no intervalo semiaberto.

## Validação

Esta primeira snapshot contém testes unitários, mas build e testes ainda aguardam execução em CI. Consulte [`docs/BUILD_CI.pt-BR.md`](docs/BUILD_CI.pt-BR.md) para comandos e geração do artefato Linux x86_64. Não considere a aplicação validada antes de um job CI concluir com sucesso.
