# Contributing

Thanks for looking at the project. This repository is the server of an unofficial, personal and educational private server for **Path of Exile 2 client 0.5.5e**. Read the [disclaimer in the README](README.md#disclaimer) first: it is not affiliated with Grinding Gear Games, no game files are distributed, and nothing here is meant for official servers.

The project is spread over several repositories; the overview is [docs/overview.md of the hub](https://github.com/Distorbed678/Poe2ModdingProject/blob/main/docs/overview.md), and the hub's [CONTRIBUTING.md](https://github.com/Distorbed678/Poe2ModdingProject/blob/main/CONTRIBUTING.md) has the long form of everything below (documenting reverse-engineering findings, tests that need the game, the other repositories' checks).

## Ground rules

A change that breaks one of these is not accepted (the long form is the hub's [CLAUDE.md](https://github.com/Distorbed678/Poe2ModdingProject/blob/main/CLAUDE.md)).

- **No game data in git.** No game files, bundles, extracted tables, datamined dumps, official-server captures, `Client.txt` excerpts or screenshots of them. Test fixtures must be your own synthetic or anonymised data.
- **Official servers are read-only.** Do not add anything that talks to official servers or changes a running official session.
- **No hardcoding.** Opcodes live in `modules/protocol/opcodes/<version>/*.toml`; the supported builds in `modules/protocol/versions/<version>.toml`; ports, hosts, leagues and start areas in `config/server.toml`; classes, areas and items come from the game data (`poe-data`); subsystems sit behind traits.
- **No copied code from AGPL reference projects** (KiwiNet, exileSniffer, LibGGPK3 and others). Read them to understand a format, do not copy code from them.
- **No secrets or personal data** in code, config, docs, logs or commit messages: passwords, tokens, keys, e-mail addresses, home directories, LAN addresses.
- Security problems go through [SECURITY.md](SECURITY.md), not a public issue.

## Setting up

Needs rustup (the nightly in `rust-toolchain.toml` is installed for you), `git`, a C toolchain, and Node 22 only to rebuild the web admin.

```sh
git clone --recursive https://github.com/Distorbed678/Poe2Server.git
cd Poe2Server
cargo build && cargo test
cargo xtask openapi --check
```

Tests that need the game files (`poe-data`) skip themselves when `GameFiles/` is missing, so a plain clone builds and tests. The development workflow with the game client (deploying the hook, unattended tests, the developer config) is in the hub: clone Poe2ModdingProject with `--recursive`; this repo is its `modules/server`, and `cargo xtask server run` starts the server from the hub with the right working directory.

## Branches and pull requests

1. Fork the repository (or branch, if you have write access) and create a topic branch from `main`.
2. Keep a change focused: one fix or feature, with its tests and docs.
3. Open the pull request against `main` and fill in the template. Say what changed and why, and how you checked it. A change that needs the game running says so, and what you saw.
4. CI must be green: `cargo build --locked`, `cargo test --locked`, `cargo xtask openapi --check`, a gitleaks scan of the files.

Do not reformat files you are not changing; most of the code is not `rustfmt`-formatted.

A change that spans repos is made bottom-up: commit and push (or open the pull request) in the submodule repository first (`modules/protocol`, `modules/client-hook`, `modules/web-admin`), then commit the new submodule pointer here (`git add modules/<name>`). The hub points at this repository the same way.

## Tests

A behaviour change comes with a test that fails without it. Wire formats are tested with byte arrays you built yourself or anonymised ones (never a raw capture). Movement and entity behaviour use the fixtures in `crates/poe-game/tests/fixtures/`. A change to the web API updates `docs/openapi.json` (`cargo xtask openapi`); CI fails when it is stale, and the web admin and the launcher generate their types from it.

## Documentation goes with the code

When you change behaviour, update the doc that describes it in the same change: a config key (the key's comment in `config/server.toml` and `config/server.example.toml`), hosting and release docs in `docs/` here, architecture and protocol docs in the hub's `docs/`.

## Releases

Maintainers cut releases from tags `server-v<version>`: [docs/releasing.md](docs/releasing.md). The workspace version in `Cargo.toml` must match the tag.

## Licence

By contributing you agree that your contribution is licensed under the repository's licence: MIT ([LICENSE](LICENSE)).
