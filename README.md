# bevy-personal-test

![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)
![Rust](https://img.shields.io/badge/rust-stable-orange.svg)
![Bevy](https://img.shields.io/badge/engine-Bevy-lightgrey.svg)
![Target](https://img.shields.io/badge/target-WASM%20%2B%20native-green.svg)

A deterministic multiplayer game **framework** in Rust: **Bevy** for rendering
and **Rhai** for game logic, where the logic runs inside a sandboxed script VM
so that a server and a second in-browser VM can independently re-execute it and
compare state hashes.

The anti-cheat and determinism plumbing is built first; the game that would sit
on top of it is not. See [Status](#status).

## Status

| Working today | Not built yet |
| --- | --- |
| Client boot pipeline, WASM + native, welcome screen | Any gameplay |
| Determinism primitives, state hashing, script sandbox | Game logic — 3 files carry `TODO(game-logic)` |
| Rollback/replay, authoritative loop, shadow-VM plumbing | Shipped `.rhai` scripts — `scripts/rhai/` is empty |

32 crates across `engine/` (12), `server/` (8), `client/` (3), `build/` (4),
`tools/` (6).

## Demo

Three ways to run this without a checkout-and-configure session.

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

Sections 1 and 2 cover `SoftF32` bit-reproducibility and `DeterministicRng`
state restore. Section 3 supplies its own minimal game logic, because
`AuthoritativeSimulation::step_full` deliberately leaves that as an insertion
point.

### Desktop — native window

Prebuilt binaries for Linux, macOS (Apple silicon) and Windows are attached to
each [release](https://github.com/XX-Batsu/bevy-personal-test/releases). Unpack
and run `native_dev`; keep `assets/fonts/` beside the executable or the CJK line
will not render. From a checkout, `just dev-native` does the same.

## How it fits together

Two independent re-executions of the same logic, compared by hash. The client
predicts, the server is authoritative, and a shadow VM in a separate Web Worker
audits sampled frames from inside the browser.

```mermaid
flowchart LR
    subgraph BROWSER["Browser"]
        direction TB
        JS["client/js<br/>bootstrap · transport"]
        LOADER["wasm_loader<br/>entry · ECDH handshake"]
        RENDER["bevy_runtime<br/>render · input"]
        BRIDGE["vm_bevy_bridge<br/>EcsMirror"]
        VM["vm_runtime<br/>Rhai sandbox"]
        SHADOW["wasm_shadow_worker<br/>shadow VM"]

        JS --> LOADER --> RENDER --> BRIDGE --> VM
        VM -. "sampled frames" .-> SHADOW
        SHADOW -. "state-hash mismatch" .-> BRIDGE
    end

    subgraph SERVER["Server"]
        direction TB
        GW["gateway<br/>WebSocket · framing"]
        KX["key_exchange<br/>X25519 · HKDF"]
        SIM["simulation<br/>AuthoritativeSimulation"]
        VAL["validator · anti_cheat<br/>state-hash checks"]
        REPLAY["replay_engine<br/>record · playback"]

        GW --> KX
        GW --> SIM --> VAL
        SIM --> REPLAY
    end

    LOADER -- "encrypted netcode · AES-256-GCM" --> GW
    GW -- "snapshots · corrections" --> LOADER
    VM -. "same logic, re-executed" .-> SIM
```

### One simulation frame

Every box below is deterministic: same inputs and seed produce a bit-identical
`blake3` hash on any platform. The game-logic slot is the part that is still
empty.

```mermaid
flowchart TB
    IN["Collect inputs<br/>per entity, tick-stamped"]
    TICK["Advance tick<br/>60 Hz fixed timestep"]
    LOGIC["Game logic — NOT IMPLEMENTED<br/>Rhai script, 2 ms budget"]
    RNG["Consume DeterministicRng<br/>PCG-XSH-RR"]
    HASH["compute_state_hash<br/>blake3 over tick + rng + entities"]
    CMP{"Hashes agree?"}
    OK["Commit frame"]
    ROLL["Rollback + re-simulate<br/>netcode RollbackManager"]

    IN --> TICK --> LOGIC --> RNG --> HASH --> CMP
    CMP -- yes --> OK
    CMP -- no --> ROLL --> TICK
```

## Workspace

Four layers, read top to bottom: each layer may depend on the ones below it,
never the reverse. The bottom layer is what the determinism rules are enforced
against.

```mermaid
flowchart TB
    L4["<b>entry points</b> — client/ · server/<br/>wasm_loader · wasm_shadow_worker<br/>gateway · simulation · validator · anti_cheat<br/>replay_engine · key_exchange · hot_update_cdn"]
    L3["<b>integration</b> — engine/<br/>bevy_runtime · vm_bevy_bridge · netcode<br/>shadow_vm · protocol · ota · asset_manifest"]
    L2["<b>sandbox + crypto</b> — engine/<br/>vm_runtime · crypto"]
    L1["<b>determinism core</b> — engine/<br/>deterministic · bridge_types · state_hash"]

    L4 -- "depends on" --> L3 -- "depends on" --> L2 -- "depends on" --> L1
```

Determinism rules are enforced against L1–L3 only; `bevy_runtime` sits in L3 but
is exempt, because the render layer is allowed native floats.

`build/` holds compile-time tooling (`bytecode_compiler`, `asset_encryptor`,
`cfg_mutator`, `build_utils`); `tools/` holds CLIs (`demo_cli`,
`replay_debugger`, `state_inspector`, `vm_disassembler`, `manifest-gen`,
`dev-asset-server`) plus the determinism and golden-hash scripts.

<details>
<summary><b>Crate reference</b> — what each one is responsible for</summary>

| Crate | Layer | Purpose |
| --- | --- | --- |
| `deterministic` | L1 | Determinism primitives — `SoftF32`, `DeterministicRng`, `Clock` |
| `bridge_types` | L1 | Shared VM–ECS types — `EcsMirror`, `DeterministicValue` |
| `state_hash` | L1 | blake3 state hashing |
| `vm_runtime` | L2 | Sandboxed Rhai runtime with per-frame budgets |
| `crypto` | L2 | AES-256-GCM, X25519, Ed25519, HKDF |
| `vm_bevy_bridge` | L3 | VM–ECS bridge — `BridgePlugin`, flush systems |
| `bevy_runtime` | L3 | Bevy integration — plugins, camera, asset import |
| `netcode` | L3 | Rollback netcode + deterministic replay |
| `shadow_vm` | L3 | Shadow VM anti-cheat verification |
| `protocol` | L3 | Network protocol definitions and framing codec |
| `asset_manifest` · `ota` | L3 | Manifest parsing, OTA diffing and update management |
| `wasm_loader` | L4 | Browser entry point and key exchange |
| `wasm_shadow_worker` | L4 | Shadow VM Web Worker |
| `gateway` · `key_exchange` | L4 | WebSocket gateway, ECDH handshake |
| `simulation` · `validator` · `anti_cheat` | L4 | Authoritative loop and state-hash validation |
| `replay_engine` · `hot_update_cdn` | L4 | Replay record/playback, OTA bytecode distribution |

</details>

## Build pipeline

Scripts and assets never ship in the clear, and the WASM binary is reshaped at
build time so that two releases do not share a control-flow graph.

```mermaid
flowchart LR
    subgraph SCRIPTS["Game logic"]
        RHAI[".rhai source"] --> BC["bytecode_compiler"] --> ENC1["AES-256-GCM<br/>+ Ed25519 sign"] --> BCFILE[".rhai.bc"]
    end

    subgraph BIN["Client binary"]
        RS["Rust · wasm32"] --> WB["wasm-bindgen"] --> WO["wasm-opt -Oz<br/>+ wasm-mutate CFG obfuscation"] --> DIST["dist/"]
    end

    subgraph ASSETS["Assets"]
        RAW["fonts · images · audio"] --> MG["manifest-gen"] --> ENC2["asset_encryptor"] --> MANIFEST["manifest.ron<br/>+ .enc"]
    end

    BCFILE --> OTA["hot_update_cdn<br/>OTA diff by blake3"]
    MANIFEST --> OTA
```

## Quick Start

```bash
just download-fonts      # first run: fetch CJK fonts (embedded at compile time)
just manifest            # first run: generate the asset manifest
just dev-native          # native dev mode (recommended for daily iteration)
just dev                 # WASM dev mode (full browser pipeline)
just demo                # terminal demo (determinism / hashing / sandbox)
just stop                # stop all background services
```

```bash
cargo test                    # unit tests
cargo fmt --check             # formatting
cargo clippy -- -D warnings   # lint
```

`justfile` targets zsh and macOS paths; CI uses `tools/ci_fonts.sh` instead.

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

<details>
<summary><b>Determinism rules</b> — enforced by <code>tools/check_determinism.sh</code> in CI</summary>

- No `HashMap`/`HashSet` in game logic — `BTreeMap`/`BTreeSet`/`IndexMap` only
- No native `f32`/`f64` in game logic — `SoftF32` only
- No `std::time` in state computation — frame counters and the `Clock` trait
- All randomness through the seeded `DeterministicRng`
- Deterministic iteration order everywhere; `#[repr(C)]` for serialized state
- No async in the simulation step

The scan covers `deterministic`, `state_hash`, `vm_runtime`, `netcode`,
`vm_bevy_bridge`, `shadow_vm`, `bridge_types` and `crypto`. `bevy_runtime` is
excluded: the render layer is allowed native floats. Per-file exemptions are
listed with a reason in the script.

</details>

<details>
<summary><b>WASM constraints</b></summary>

- No `std::thread` — Web Workers via `wasm-bindgen`
- No `std::time::Instant` — `web_sys::Performance` instead
- No `std::fs` — all assets fetched over HTTP and decrypted in memory
- `panic = "abort"` in release; `tracing` macros instead of `println!`
- Total WASM memory ≤ 512 MB

</details>

<details>
<summary><b>Security model</b></summary>

- Rhai scripts run in a sandbox: no filesystem, network, or `eval`; opcode
  budgets and scope limits are enforced per frame
- Assets and script bytecode are AES-256-GCM encrypted, Ed25519 signed, with
  session keys derived via X25519 ECDH + HKDF-SHA256
- A shadow VM in a separate Web Worker re-executes sampled frames and
  compares blake3 state hashes for anti-cheat validation

</details>

<details>
<summary><b>Asset management</b></summary>

External binary assets (fonts, images, audio) are not committed to git; they
are fetched by `just` recipes (e.g. `just download-fonts`) and, where needed
at compile time via `include_bytes!`, guarded by `build.rs` checks that fail
fast with an actionable message.

</details>

## License

MIT — see [LICENSE](LICENSE).

This project is an independent work built on the [Bevy](https://bevyengine.org)
game engine and is not affiliated with or endorsed by the Bevy Foundation.
