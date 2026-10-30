# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`seoul` is a MilkDrop-inspired real-time audio visualizer in Rust (wgpu 30, cpal 0.18, winit 0.30). It captures desktop audio via WASAPI loopback (or a microphone / line-in, with room tuning), extracts features (AGC'd bands, beats, tempo, spectrum, waveform), and renders feedback-driven HDR shader visuals. Presets are hot-reloadable TOML + WGSL. Developed on Windows (WASAPI); macOS 14.6+ builds in CI but is untested on hardware (CoreAudio process-tap loopback, OS fonts per platform in `render/hud.rs::os`, bundle paths in `places.rs::mac_support_dir`). Linux (x86_64, and aarch64 for the Raspberry Pi 5 frame) builds in a Debian bookworm Docker box; it has no loopback, so it listens to inputs.

Deeper docs: `docs/ARCHITECTURE.md` (threads, analysis math, pass graph, transitions, invariants) and `docs/PRESETS.md` (authoring guide).

## Commands

```bash
cargo run --release        # primary — debug builds are too slow for 60 fps
run.bat / .\run.ps1        # same, from any cwd; extra args pass through
cargo test                 # unit tests live alongside the code
cargo test <name>
RUST_LOG=seoul=debug cargo run --release

# Visual verification (no music needed) — check every visual change this way:
cargo run --release -- --synth --tour 10 --tour-shots 3 --size 1280x720   # → screenshots/tour/, then exit
cargo run --release -- --synth --tour 10 --preset ink                     # --preset filters a tour by name
cargo run --release -- --synth --preset Vortex --screenshot-at 5
cargo run --release -- --room-sim          # the synth as a mic across the room hears it; --no-room for untuned
cargo run --release -- --mic               # or --input <NAME>; --list-inputs shows names
cargo test --release room_report -- --ignored --nocapture   # room tuning's numbers at three mic gains
cargo run --release -- --frame --evening --clock 22:59     # frame mode through a scripted evening (talk, a record) into sleep
cargo test --release mood_report -- --ignored --nocapture  # frame mode's mood timeline for that evening
cargo run --release -- --help

# Platforms and releases
cargo check --release --target aarch64-apple-darwin --target-dir target/macos   # Mac compile check from Windows
.\packaging\package.ps1          # Windows zip → dist/
docker build -t seoul-linux -f packaging/linux.Dockerfile packaging              # once
docker run --rm -v "$PWD:/src" -w /src seoul-linux ./packaging/package-linux.sh aarch64   # Pi 5 tarball → dist/ (no arg: x86_64)
cargo run --release -- --synth --bench 8 --size 720x1280   # fps per preset, vsync off → screenshots/bench/*.csv
WGPU_BACKEND=gl ...                # try OpenGL ES instead of Vulkan
git tag vX.Y.Z && git push --tags  # CI (.github/workflows/build.yml) drafts a release with every package
```

`--synth` is a deterministic 124 BPM test track (`src/audio/synth.rs`). Screenshots re-run the display passes offscreen and can be viewed with the Read tool. When a script launches the app, send its output to a file, not a pipe: a full pipe blocks the render thread on logging.

Keys: Space/Backspace next/prev · R shuffle · A auto-advance · L lock · F favorite · X hide · 1–9 favorites · H help · F1 stats · P screenshot · F11 fullscreen · Esc quit. Touch (`touch.rs`): tap = next, two-finger tap = quit.

Settings: `seoul.toml` (hot-reloads; CLI overrides). Favorites/hidden: `seoul-state.toml` (gitignored). Both live in the working folder, except under `$EASEL_DATA` or a Linux install (`~/.config/seoul`); see `places.rs`. SIGTERM/SIGINT quit through `App::quit`, like Esc.

## Architecture in brief

- **Threads:** WASAPI callback → ring → analysis thread (`audio/analysis.rs`, pure `Analyzer` on a 256-sample hop; feeds itself real-time zeros when loopback goes quiet) → triple buffer → render thread. `Capture` (main thread, polled per frame) opens a `Source` (loopback, default input, or an input by name) and reconnects on stream errors and default-device changes.
- **Frame mode** (`frame.rs`, `--frame` / `[frame]`): `MoodTracker` reads resting / listening / playing from the features (music = unbroken sound, sustained sound, a confident groove, or a playing link); `FrameMode` eases a `Look` (scene `speed`, exposure, what presets hear, night `warmth`, sleep fade) that `main.rs` applies via `frame::apply` and `Renderer::set_look`. Details: `docs/ARCHITECTURE.md#frame-mode`.
- **Room tuning** (`Analyzer::set_room`, on for inputs): a per-bin noise floor subtracted before anything reads the FFT, and a gain that puts the room's noise just under the −50 dBFS floor (lifting a quiet mic, cutting a hot one), so the analysis's absolute thresholds mean "above the room". `presence` gates volume, waveform and onsets. Tested against `RoomSim` (`audio/synth.rs`) at three mic gains. Details: `docs/ARCHITECTURE.md#room-tuning`.
- **Soba link** (`audio/link.rs`, spec in `docs/LINK.md`): OSC on `127.0.0.1:9137` → `seoul-link` thread → timestamped snapshots → merged into the render thread's copy of `AudioFeatures` after `[link] delay_ms`. It adds `cv1..8`, `gate1..4` (+ `_age`, `_count`), `link_live`, `playing` and `bar`, and Soba's clock overrides tempo detection while playing. `--synth` runs a stand-in Soba (`audio/link_fake.rs`) over real UDP; `--record` folds it in directly; `--no-link` turns both off; `cargo run --example link_send` sends it alone.
- **Time is stamped on the render thread** (`time`, `dt`, `frame`, `aspect`, `resolution`), never in analysis.
- **Frame:** warp (per-pixel, per active preset) → particles (compute + sprites, optional) → composite (additive `fs_composite`) → bloom → post (mirror/echo, PBR Neutral tonemap, grade) → HUD. Two ping-pong `Rgba16Float` feedback textures follow the window size.
- **Frame-rate independence:** presets are authored per frame at 60 Hz; `WarpParams::to_uniforms` and the composite mask rescale by `k = dt·60`.
- **Transitions** are per-pixel: `render/mask.rs` writes each preset's weight into the feedback **alpha** channel before its warp/composite draw; those pipelines use `MASKED_BLEND` (`src × DstAlpha`, color-only writes). Presets never handle transitions.
- **Presets:** `preset/preset.rs` (parse; unknown keys rejected), `preset/expr.rs` (mapping expressions), `preset/params.rs` (`[params]`: stateful per-frame values exposed to shaders as `params()`), `preset/library.rs` (navigation, curation, auto-advance, `FramePlan`, debounced hot reload with add/remove), `preset/shader.rs` (compile with prelude; **both** shader-module and pipeline creation inside wgpu error scopes so bad presets return `Err`; naga line numbers remapped to the author's file).

## Invariants

- `AudioFeatures` (`src/audio/features.rs`) is both the CPU snapshot and the GPU storage-buffer layout; its WGSL twin is in `shaders/common.wgsl`. Keep them field-for-field identical (a size assert guards drift). The same applies to `WarpUniforms`, `PostUniforms`, `ParticleUniforms`, `PresetUniforms` (palette + `[params]`, WGSL `Palette`) and the mask uniforms versus their WGSL structs.
- Only the mask pass may write feedback alpha; anything else drawn into feedback uses `ColorWrites::COLOR`.
- Anything compiled from user files must go through an error scope.
- Per-frame motion/energy authored at 60 Hz must be scaled by `k = dt·60`.

## Audio capture gotcha (WASAPI loopback)

cpal's WASAPI backend implements loopback via a specific pattern: call `build_input_stream` **on the default output device**, using the config returned by `default_output_config()` (not `default_input_config()`). The backend detects this combination and sets `AUDCLNT_STREAMFLAGS_LOOPBACK`. If capture ever silently stops working, re-check the `Source::Loopback` arm of `src/audio/capture.rs::open`; the device/config pairing is the load-bearing part. This is still true as of cpal 0.18, which also reports `StreamInvalidated`/`DeviceNotAvailable` on default-device changes; `Capture` uses those to reconnect.

A muted mic (laptop mute key, privacy shutter, Sound settings) delivers exact digital zeros, not an error: the HUD then says "no signal — is the mic muted?" and the log warns. `RUST_LOG=seoul=debug` logs the input's peak level every 5 s. Default configs may now be I32/F64/I24; the generic `push_mono` handles them all.

<!-- claims:begin (from chrischaps/Dashboard skills/setup-claims; re-run its install.sh to update) -->
## Claiming issues

Agents on several machines work this repo, all as the same GitHub user, so an issue is claimed before anyone works on it. A claim is the branch `claim/issue-<n>` on origin, holding one empty commit that names the agent and host. Git accepts only the first push of a new branch, so exactly one claimant wins. `scripts/issue.sh` does the whole cycle (bash; on Windows it runs under Git Bash):

```bash
scripts/issue.sh list                  # who holds what, on every machine, and for how long
scripts/issue.sh claim <n>             # claim and make the worktree ../Seoul-<n>; exit 2: taken, pick another
scripts/issue.sh claim <n> --here      # claim for the worktree you're already in (a desktop-app worktree)
scripts/issue.sh renew <n>             # at the start of each session on long work
scripts/issue.sh land <n>              # verify, catch up with main, fast-forward push to it, release the claim
scripts/issue.sh release <n>           # giving up: push your branch and comment where it stands first
```

- **A hook enforces it.** `.claude/settings.json` runs `scripts/issue-guard.sh` before every `git commit` and `git push`. It refuses issue work (a branch named `issue-<n>`, or a commit saying `Closes #<n>`) unless this session holds the claim, from the worktree it was claimed for. A chore with no issue (a typo, a dependency bump) needs no claim: keep it off `issue-<n>` branches and out of `Closes` lines. A SessionStart hook lists the current claims.
- **Only the session that claimed an issue renews, releases or lands it,** and only from its worktree. An existing worktree for an issue you didn't claim is another session's: leave it alone. Never delete another agent's `claim/*` branch.
- **Claims go stale after 24 hours.** `claim <n> --steal` takes over a stale one; check the issue's comments and `origin/issue-<n>` for work in progress first, and continue from it. Taking a younger claim is Chris's call only: `claim <n> --steal --override "<his reason>"`.
- Settings for this repo are in `scripts/issue.conf`. The tool is vendored from `skills/setup-claims` in chrischaps/Dashboard: change it there, not here. `bash scripts/issue-test.sh` tests it against a throwaway remote.
<!-- claims:end -->
