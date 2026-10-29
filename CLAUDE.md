# CLAUDE.md

Poe2Server is the server of the Poe2ModdingProject family (Path of Exile 2, client 0.5.5e Hotfix 1). The project rules (hard rules, official servers read-only, no hardcoding, docs with code, commit and push after each step) and the roadmap are in the hub's CLAUDE.md:
<https://github.com/Distorbed678/Poe2ModdingProject/blob/main/CLAUDE.md>. In the hub checkout this repo is `modules/server` and that file is `../../CLAUDE.md`.

Specific to this repo:

- It must build and test on its own: `git clone --recursive`, `cargo build && cargo test`, `cargo xtask openapi --check`. Do not add a dependency on anything outside this repo and its submodules (`modules/protocol`, `modules/client-hook`, `modules/web-admin`).
- Paths in the config and in the version files are relative to the working directory: this repo's root, or the hub's root when the hub runs the server (`cargo xtask server run`).
- The supported client builds are defined in `modules/protocol/versions/<version>.toml`; opcodes in `modules/protocol/opcodes/<version>/`; ports, leagues, start areas in `config/server.toml`. No hardcoding.
- Documentation of architecture, protocol and reverse engineering lives in the hub's `docs/`; hosting and release docs live here (`docs/`). Update them in the same commit as the code.
- Releases: tag `server-v<version>` (`docs/releasing.md`). The workspace version in `Cargo.toml` must match.
- Submodules: commit and push inside the submodule first, then bump the pointer here.
