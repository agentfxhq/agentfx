# Contributing

Thanks for your interest in AgentFX.

- **Issues:** bug reports with a minimal `.reel` script (or IR JSON), the command you ran,
  the output of `agentfx --version`, your OS and GPU, and a frame or sheet PNG if the
  problem is visual.
- **Pull requests:** keep them small and focused. Run `cargo build --release` and
  `cargo test --release` at the repository root before you open one; tests that need
  ffmpeg or a hardware encoder skip themselves when it is missing.
- **New features:** open an issue first. A new element, verb or effect also needs an entry
  in the capability catalog (`agentfx catalog`), a line in `docs/SPEC.md`, an example under
  `examples/features/` and a test.
- **Determinism:** same script, same frames. Seed every random choice and do not read the
  clock, the network or the environment during a render.
- **Vendored crates:** changes in `vendor/` are marked with a `reel patch` comment and
  listed in `vendor/README.md`.
- **Style:** `cargo fmt` is not enforced on existing code; match the surrounding style.
  Docs and UI text without em-dashes.

By contributing you agree that your contribution is licensed under the Apache License 2.0
(see `LICENSE`), as stated in section 5 of that licence.
