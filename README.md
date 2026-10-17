# Motion Editor

An experimental native motion-editing prototype written in Rust for Linux x86_64. It has its own versioned project model and file format; it does not open Alight Motion projects or run its APK.

## Current vertical slice

- Create a 9:16 project at 1080×1920 and 30 fps.
- Import PNG/JPEG still images and place them as five-second clips.
- Preview frames with a CPU compositor, including position, scale, rotation, opacity, and linear/ease-in/ease-out/ease-in-out keyframes.
- Save and reopen versioned JSON project files (`.mproj`; extension is provisional).
- Exercise an export-job adapter with immutable snapshots, a bounded queue, cancellation, progress, and atomic output publication.

## Current limits

There is no video or audio decoder and no MP4/H.264/AAC encoder. Export reports an explicit missing-encoder error; this snapshot cannot produce a finished video. The initial interface and project workflow are incomplete, and the project format is still evolving.

This is a clean-room implementation. It does not contain Alight Motion code, assets, branding, or project data. FilmCraft has not been added as a code dependency.

## Build and CI

See [the Linux x86_64 CI build guide](docs/BUILD_CI.pt-BR.md) for the Rust toolchain, system packages, commands, and expected release-binary path. There is no CI workflow configured in this repository yet.

An x86_64 Linux executable and its dependency lockfile have been supplied and inspected. The archive did not include CI test logs, so test status remains unverified. This repository does not yet contain an automated CI workflow.

## Project notes

The implementation contract and module breakdown are in [IMPLEMENTACAO.md](IMPLEMENTACAO.md) (Portuguese). Contributor boundaries are in [AGENTS.md](AGENTS.md).

No license has been selected yet; public visibility alone does not grant permission to redistribute or reuse the code.
