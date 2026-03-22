# bevy-personal-test

A multiplayer action game experiment built with **Bevy** (WASM target) and the
**Rhai** scripting engine, featuring a fully deterministic simulation designed
for server-authoritative validation and rollback netcode.

> Work in progress — the codebase is being built up phase by phase.

## Highlights

- Deterministic game logic: software floats (`SoftF32`), seeded PCG RNG,
  fixed 60 Hz timestep — no native float, no `HashMap`, no system time in
  simulation code
- Sandboxed Rhai scripting with per-frame execution budgets
- blake3 state hashing for cross-client verification
- Encrypted assets and bytecode (AES-256-GCM, X25519 ECDH, Ed25519)

## Build

```bash
cargo test          # unit tests
cargo fmt --check   # formatting
cargo clippy -- -D warnings
```

## License

MIT — see [LICENSE](LICENSE).

This project is an independent work built on the Bevy game engine and is not
affiliated with or endorsed by the Bevy Foundation.
