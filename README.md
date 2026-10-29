# Poe2Server

The private server for **Path of Exile 2 (client 0.5.5e Hotfix 1)**, written in Rust: login server, instance server (one shared world per area), the web API and the web admin, accounts on SQLite, and a mod host for native server mods. It is a personal, educational project; it never touches official servers.

This repository is one part of the Poe2ModdingProject family. The hub, [Poe2ModdingProject](https://github.com/Distorbed678/Poe2ModdingProject), holds the documentation, the plans, the reverse-engineering notes and the dev tooling, and has this repo as the submodule `modules/server`. The overview of all repositories and how they depend on each other: [Project overview](https://github.com/Distorbed678/Poe2ModdingProject/blob/main/docs/overview.md).

## Get it

- **Play or host without a terminal:** the [Poe2Launcher](https://github.com/Distorbed678/Poe2Launcher) installs the client and can download and run a server release.
- **Release packages:** [Releases](https://github.com/Distorbed678/Poe2Server/releases) (tags `server-v<version>`; Linux `.tar.gz`, Windows `.zip`, SHA-256 checked). How to run one: [docs/hosting.md](docs/hosting.md).
- **From source:** below.

## Build from source

Needs Linux or Windows with rustup (the nightly pinned in `rust-toolchain.toml` is installed for you) and, to rebuild the web admin, Node 22. No game files are needed to build or test; tests that read the game skip themselves when `GameFiles/` is absent.

```sh
git clone --recursive https://github.com/Distorbed678/Poe2Server.git
cd Poe2Server
cargo build && cargo test
cargo xtask openapi --check        # docs/openapi.json is current
```

Run it from the repository root, because every path in the config is relative to the working directory:

```sh
mkdir -p runtime && cp config/server.example.toml runtime/server.toml     # edit: game_dir, [network] public_host, ...
POE_CONFIG=runtime/server.toml cargo run --release -p poe-server
```

`config/server.toml` is the committed, safe-by-default config (loopback only); a shared server starts from `config/server.example.toml`. The web admin (`modules/web-admin`, Poe2WebAdmin) is served from `[web] static_dir`: `cd modules/web-admin && npm ci && npm run build`. The server logs which build it serves (`dist/build.json`) and warns when the sources are newer.

## Layout

| Path | Contents |
|---|---|
| `servers/poe-server` | The binary: runs the login and instance servers and the web API in one process; `account`, `character` and `mod` command lines |
| `servers/login-server`, `servers/instance-server` | Login (accounts, characters, hand-off) and the instance actor world (movement, entities, chat, party, trade) |
| `crates/` | `poe-config` (typed config, version files), `poe-data` (game data and schema), `poe-world` (terrain generator), `poe-game` (movement and simulation rules), `poe-store`, `poe-accounts`, `poe-cluster`, `poe-web-api` (HTTP/WebSocket API, OpenAPI), `poe-mod-host` |
| `config/` | `server.toml` (the safe config), `server.example.toml` (commented template for hosts) |
| `docs/` | [hosting.md](docs/hosting.md), [releasing.md](docs/releasing.md), `openapi.json` (written by `cargo xtask openapi`) |
| `xtask/` | `cargo xtask openapi [--check]` |
| `modules/protocol` | [Poe2Protocol](https://github.com/Distorbed678/Poe2Protocol): wire protocol crates, `opcodes/<version>/` and `versions/<version>.toml` (the supported client builds) |
| `modules/client-hook` | [Poe2ClientHook](https://github.com/Distorbed678/Poe2ClientHook): `poe-hook-shared` (side-channel framing) and `poe-hook-mods` (mod manifest, shared with the launcher) |
| `modules/web-admin` | [Poe2WebAdmin](https://github.com/Distorbed678/Poe2WebAdmin): the web admin site, packaged into releases |

Server mods are written with [Poe2ModSDK](https://github.com/Distorbed678/Poe2ModSDK); the server pins its `poe-mod-api` crate by tag.

## Documentation

The architecture, the protocol, the reverse-engineering notes, the guides (setup, server admin, modding) and the plans are in the hub: [docs/index.md](https://github.com/Distorbed678/Poe2ModdingProject/blob/main/docs/index.md). Comments in the code that mention `docs/...` mean the hub's docs folder. The rules for working on the project are in the hub's [CLAUDE.md](https://github.com/Distorbed678/Poe2ModdingProject/blob/main/CLAUDE.md); this repo's [CLAUDE.md](CLAUDE.md) points there.

## Disclaimer

This is an unofficial, fan-made project for personal and educational use. It is **not affiliated with, endorsed by or connected to Grinding Gear Games**; "Path of Exile" is their trademark. **No game files are distributed** here (no assets, bundles, tables or executables): you need your own legal copy of the game, and the server only reads it from your machine. You use all of this at your own risk. More: [SECURITY.md](SECURITY.md), [CONTRIBUTING.md](CONTRIBUTING.md); licensed under MIT ([LICENSE](LICENSE), Copyright (c) 2026 Distorbed678 and contributors).
