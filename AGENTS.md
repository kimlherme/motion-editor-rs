# Contributor guidance

- Keep this a native Rust application targeting Linux x86_64 for the initial release.
- Maintain an independent implementation and project format. Do not copy or include Alight Motion code, assets, branding, APK contents, or proprietary project data.
- FilmCraft may inform architecture, but do not copy code or add it as a dependency until its license and provenance have been reviewed and documented.
- Follow the shared contracts in `IMPLEMENTACAO.md`. Discuss changes to cross-crate APIs before changing their semantics.
- Keep media and export capabilities explicit. Do not claim video/audio decoding or MP4/H.264/AAC encoding until a backend exists and has passed CI verification.
- Run formatting, tests, and a release build in CI before describing a change as validated. Report checks that could not be run.
