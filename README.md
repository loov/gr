# gr

A git history viewer in Rust and egui, based on gitk.

- `src/lib.rs` – UI-free core: history loading, graph layout, commit details and
  diffs, find, goto, history. Shells out to `git`.
- `src/main.rs` – the egui UI.
- `fonts/` – Lyth Mono (SIL OFL), embedded for all text.
- `vendor/winit/` – winit 0.30.13 (Apache-2.0), patched so the macOS
  press-and-hold accent menu works in text fields.

Run with `cargo run --release -- [repo] [git log args] [-- paths]`, pinned to rustc 1.95 via `rust-toolchain.toml`.

## License

GPL-3.0-or-later. gitk, which it is based on, is GPL-2.0-or-later. See `LICENSE`.
Fonts and vendored winit keep their own licenses.
