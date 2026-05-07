# bevy-personal-test

![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)
![Rust](https://img.shields.io/badge/rust-stable-orange.svg)
![Bevy](https://img.shields.io/badge/engine-Bevy-lightgrey.svg)
![Target](https://img.shields.io/badge/target-WASM%20%2B%20native-green.svg)

A multiplayer action game built with **Bevy** (WASM target) and the **Rhai**
scripting engine. Game logic runs inside a fully deterministic, sandboxed
script VM, enabling server-authoritative validation, rollback netcode, and
anti-cheat verification via a shadow VM.

## Quick Start

```bash
just download-fonts      # first run: fetch CJK fonts (embedded at compile time)
just manifest            # first run: generate the asset manifest
just dev-native          # native dev mode (recommended for daily iteration)
just dev                 # WASM dev mode (full browser pipeline)
just stop                # stop all background services
```

```bash
cargo test                    # unit tests
cargo fmt --check             # formatting
cargo clippy -- -D warnings   # lint
```

## Architecture

```
┌─────────────────────────────────────────────────┐
│                   Browser                       │
│  ┌───────────┐  ┌────────────┐  ┌────────────┐  │
│  │  Bevy App │  │ Shadow VM  │  │  WASM      │  │
│  │  (WASM)   │  │ (Worker)   │  │  Loader    │  │
│  │           │  │            │  │  + Key Exch│  │
│  │ ┌───────┐ │  │ ┌────────┐ │  └────────────┘  │
│  │ │Render │ │  │ │Rhai VM │ │                  │
│  │ │Layer  │ │  │ │(verify)│ │                  │
│  │ └───┬───┘ │  │ └────────┘ │                  │
│  │ ┌───┴───┐ │  └─────┬──────┘                  │
│  │ │VM-ECS │ │        │ state hash comparison   │
│  │ │Bridge │◄├────────┘                         │
│  │ └───┬───┘ │                                  │
│  │ ┌───┴───┐ │                                  │
│  │ │Rhai VM│ │                                  │
│  │ │(main) │ │                                  │
│  │ └───────┘ │                                  │
│  └─────┬─────┘                                  │
│        │ Netcode (rollback)                     │
└────────┼────────────────────────────────────────┘
         │
    ┌────┴─────┐
    │  Server  │
    │ (validate│
    │  + sync) │
    └──────────┘
```

## Technical Decisions

| Area                | Choice                                              |
| ------------------- | --------------------------------------------------- |
| Game engine         | Bevy (WASM target)                                  |
| Script engine       | Rhai (pure Rust, sandboxed)                         |
| Hash                | blake3                                              |
| Float determinism   | `SoftF32` (game logic), native float (render layer) |
| RNG                 | PCG-XSH-RR, seeded                                  |
| Fixed timestep      | 60 Hz logic, interpolated rendering                 |
| Script budget       | 2 ms per script, 4 ms per frame                     |
| Symmetric crypto    | AES-256-GCM                                         |
| Key exchange        | X25519 ECDH + HKDF-SHA256                           |
| Signing             | Ed25519                                             |
| CFG obfuscation     | wasm-mutate (build time)                            |
| Serialization       | bincode                                             |
| WASM memory cap     | 512 MB                                              |

## Workspace Layout

### Engine (`engine/`)

| Crate            | Purpose                                                    |
| ---------------- | ---------------------------------------------------------- |
| `deterministic`  | Determinism primitives (SoftF32, DeterministicRng, Clock)  |
| `bridge_types`   | Shared VM–ECS bridge types                                 |
| `state_hash`     | blake3 state hashing                                       |
| `crypto`         | Crypto pipeline (AES-256-GCM, X25519, Ed25519, HKDF)       |
| `vm_runtime`     | Sandboxed Rhai VM runtime with per-frame budgets           |
| `vm_bevy_bridge` | VM–ECS bridge (BridgePlugin, EcsMirror, flush systems)     |
| `bevy_runtime`   | Bevy integration layer (plugins, systems, asset import)    |
| `netcode`        | Rollback netcode + deterministic replay                    |
| `shadow_vm`      | Shadow VM anti-cheat verification                          |
| `protocol`       | Network protocol definitions & framing codec               |
| `asset_manifest` | Asset manifest parsing and OTA diffing                     |
| `ota`            | OTA update management                                      |

### Server (`server/`)

`gateway`, `key_exchange`, `hot_update_cdn`, `simulation`, `validator`,
`replay_engine`, `anti_cheat`, `server_types` — WebSocket gateway, ECDH
handshake, OTA bytecode distribution, authoritative simulation, and
state-hash validation.

### Client (`client/`)

`wasm_loader` (entry point + key exchange), `wasm_shadow_worker` (shadow VM
Web Worker), `js/` (bootstrap loader).

### Build & Tools (`build/`, `tools/`)

`bytecode_compiler` (`.rhai` → encrypted `.rhai.bc`), `asset_encryptor`,
`cfg_mutator` (wasm-mutate seeds), `manifest-gen`, `dev-asset-server`,
`replay_debugger`, `state_inspector`, `vm_disassembler`, plus determinism
static-analysis and golden-hash scripts.

## Determinism Rules

All simulation code follows strict determinism rules, enforced by static
checks and CI:

- No `HashMap`/`HashSet` in game logic — `BTreeMap`/`BTreeSet`/`IndexMap` only
- No native `f32`/`f64` in game logic — `SoftF32` only
- No `std::time` in state computation — frame counters and the `Clock` trait
- All randomness through the seeded `DeterministicRng`
- Deterministic iteration order everywhere; `#[repr(C)]` for serialized state
- No async in the simulation step

## WASM Constraints

- No `std::thread` — Web Workers via `wasm-bindgen`
- No `std::time::Instant` — `web_sys::Performance` instead
- No `std::fs` — all assets fetched over HTTP and decrypted in memory
- `panic = "abort"` in release; `tracing` macros instead of `println!`
- Total WASM memory ≤ 512 MB

## Asset Management

External binary assets (fonts, images, audio) are not committed to git; they
are fetched by `just` recipes (e.g. `just download-fonts`) and, where needed
at compile time via `include_bytes!`, guarded by `build.rs` checks that fail
fast with an actionable message.

## Security Model

- Rhai scripts run in a sandbox: no filesystem, network, or `eval`; opcode
  budgets and scope limits are enforced per frame
- Assets and script bytecode are AES-256-GCM encrypted, Ed25519 signed, with
  session keys derived via X25519 ECDH + HKDF-SHA256
- A shadow VM in a separate Web Worker re-executes sampled frames and
  compares blake3 state hashes for anti-cheat validation

## License

MIT — see [LICENSE](LICENSE).

This project is an independent work built on the [Bevy](https://bevyengine.org)
game engine and is not affiliated with or endorsed by the Bevy Foundation.
