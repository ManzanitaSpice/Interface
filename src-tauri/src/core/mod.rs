// ─── InterfaceOficial Core ───
// Modular backend architecture for a professional Minecraft launcher.
//
// Architecture:
//   core/
//     instance/   — Instance model + CRUD manager
//     version/    — Mojang manifest + version JSON + OS rules
//     maven/      — Artifact parser, POM resolver, transitive deps
//     downloader/ — Concurrent downloads with SHA-1 validation
//     assets/     — Asset index + object downloads
//     loaders/    — Vanilla, Fabric, Quilt, Forge, NeoForge
//     launch/     — Classpath builder + process spawner
//     java/       — Multi-platform Java detection
//     state/      — Global application state

pub mod assets;
pub mod downloader;
pub mod error;
pub mod instance;
pub mod java;
pub mod launch;
pub mod loaders;
pub mod maven;
pub mod state;
pub mod version;
