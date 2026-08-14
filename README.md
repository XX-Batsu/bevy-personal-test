# bevy-personal-test

![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)
![Rust](https://img.shields.io/badge/rust-stable-orange.svg)
![Bevy](https://img.shields.io/badge/engine-Bevy-lightgrey.svg)
![Target](https://img.shields.io/badge/target-WASM%20%2B%20native-green.svg)

A multiplayer action game built with **Bevy** (WASM target) and the **Rhai**
scripting engine. Game logic runs inside a fully deterministic, sandboxed
script VM, enabling server-authoritative validation, rollback netcode, and
anti-cheat verification via a shadow VM.

## Demo

Three ways to run this without a checkout-and-configure session. None of them is
a playable game: the simulation exposes game-logic insertion points rather than
gameplay, so what runs is the framework, not a title built on it.

### Browser — client boot pipeline

**[Live demo](https://xx-batsu.github.io/bevy-personal-test/)** — a static WASM
build on GitHub Pages. It executes the real client startup path (WASM
instantiate → Bevy engine init → first frame) and renders the welcome screen.
No gateway server sits behind it, so the client starts in offline mode: no
WebSocket, no ECDH handshake, no netcode session. Locally, append `?offline` to
any dev URL for the same behaviour.

### Terminal — determinism, state hashing, sandbox

The `demo` CLI exercises the guarantees the framework is built around. No
assets, no server, no browser:

```bash
cargo run -p demo_cli --release                     # all four sections
cargo run -p demo_cli --release -- --only sandbox   # one section
cargo run -p demo_cli --release -- --ticks 32       # longer hash chain
```

```
== 1. SoftF32 — software float, bit-reproducible across targets ==
  expr         SoftF32        native f32     bits equal
  0.1 + 0.2    0x3E99999A     0x3E99999A     yes
  sqrt(2.0)    0x3FB504F3     0x3FB504F3     yes
  sin(1.0)     0x3F576AA4     0x3F576AA4     yes

== 2. DeterministicRng — PCG-XSH-RR, seeded and restorable ==
  seed 42, draws 1-4     : [1307692281, 3850602322, 1491967504, 4091771729]
  captured state (16B)   : d0332fa6deee39dd0300000000000000
  draws 5-8 from rng     : [3882238836, 1795024040, 2266118430, 1938801432]
  draws 5-8 from state   : [3882238836, 1795024040, 2266118430, 1938801432]  (identical: true)

== 3. State hash chain — blake3 over (tick, rng state, entities) ==
  tick   1  blake3:fb8129b25c8424be…  seed42-rerun: match  seed43: diverged
  tick   2  blake3:ad8290c72f625e5a…  seed42-rerun: match  seed43: diverged
  seed 42 replay is bit-identical: true
  seed 43 diverges from tick 1

== 4. Rhai sandbox — symbol blocking and resource limits ==
  plain arithmetic   -> allowed, result = 45
  eval()             -> rejected (CompileError)
  module import      -> rejected (CompileError)
  unbounded loop     -> rejected (Timeout)
  limits: 50,000 ops, 32 call levels, 4 KiB strings, 2 ms per script.
```

Section 3 supplies its own minimal game logic (SoftF32 motion plus RNG jitter),
because `AuthoritativeSimulation::step_full` deliberately leaves that as an
insertion point.

### Desktop — native window

Prebuilt binaries for Linux, macOS (Apple silicon) and Windows are attached to
each [release](https://github.com/XX-Batsu/bevy-personal-test/releases). Unpack
and run `native_dev`; keep `assets/fonts/` beside the executable or the CJK line
will not render. From a checkout, `just dev-native` does the same.

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
`demo_cli` (the terminal demo above), `replay_debugger`, `state_inspector`,
`vm_disassembler`, plus determinism static-analysis and golden-hash scripts.

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
