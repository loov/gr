# gr

A git history viewer in Rust and egui, based on gitk.

- `src/lib.rs` – UI-free core: history loading, graph layout, commit details and
  diffs, find, goto, history. Shells out to `git`.
- `src/main.rs` – the egui UI.
- `fonts/` – Lyth Mono (SIL OFL), embedded and used where Menlo, loaded
  from macOS, is missing.
- `themes/` – colour themes from iTerm2-Color-Schemes (MIT), in Ghostty's
  format; only those whose own licence is known and fits GPL-3.0-or-later,
  listed with licence and source in `themes/licenses.yml` and with each
  author's licence text in `themes/NOTICES.md`.
- `vendor/winit/` – winit 0.30.13 (Apache-2.0), patched so the macOS
  press-and-hold accent menu works in text fields.

Run with `cargo run --release -- [repo] [git log args] [-- paths]`, pinned to rustc 1.95 via `rust-toolchain.toml`.

`gr github pr <number> [repo]` (or `gr gh pr`) shows a GitHub pull request the way Gerrit shows
patch sets. It uses `gh` to find the head before each force-push and the
current head, fetches them into `refs/gh/pr/<number>/v1`, `v2`, ..., and
lists the commits of the newest. `git range-diff` pairs each commit with
its earlier revisions; the list marks the versions each patch is in and
whether it changed, and the bar under the diff picks the patch set to show
and the one to compare it with.

## License

GPL-3.0-or-later. gitk, which it is based on, is GPL-2.0-or-later. See `LICENSE`.
Fonts, themes and vendored winit keep their own licenses.
