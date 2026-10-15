# Soba × Seoul: Link Proposal

Oct 15, 2026 · @Chris

## Summary

Let a Soba patch play Seoul directly, so one patch makes both the sound and the picture. Soba sends its control signals (LFOs, envelopes, gates, clock) over OSC on localhost. Seoul exposes them to presets as new expression variables. Neither app depends on the other. The link is a one-page protocol, and each app implements its own side.

## Motivation

Seoul already hears Soba through WASAPI loopback, but it can only guess at intent. It infers bands, beats and tempo from the mixed signal. It can't know that a filter sweep comes from an LFO at 0.25 Hz, or that a gate opened on bar 3, or what the true tempo is.

- **Exact clock.** Seoul's tempo detector reports `bpm_confidence` because it estimates. Soba's Clock module knows the tempo exactly. Every preset that follows `beat_phase` would lock to the patch.
- **Gestures you can't hear.** A slow LFO that shapes a timbre is buried in the mix. Sent as CV, it can drive zoom, warp or palette directly.
- **One instrument.** Patching a cable in Soba becomes a way to compose the picture as well as the sound. This fits what both apps already value: signal flow you can see.

## Proposal

Keep Soba and Seoul as separate apps and connect them with a protocol, not shared code. Soba sends control data over OSC. Seoul merges it into the features its presets already read. Audio keeps reaching Seoul through loopback, exactly as today.

&#91;embedded content: Soba × Seoul link · two paths, three new parts\]

Seoul's flow reads upward: both paths meet in one `AudioFeatures` snapshot per frame, and presets map the new lanes with the expressions they already use.

## Protocol

The link is OSC 1.0 over UDP to `127.0.0.1`, default port 9137, configurable on both sides. Soba sends one OSC bundle per tick at about 200 Hz (every 5 ms). Each bundle carries every lane, so a dropped packet is repaired by the next one.

| Address | Arguments | Meaning |
| --- | --- | --- |
| `/soba/cv/1` … `/soba/cv/8` | `f` value | A Control signal, sent raw in Soba's control range |
| `/soba/gate/1` … `/soba/gate/4` | `i` 0 or 1 | A Gate's state; a rising edge is a trigger |
| `/soba/clock` | `f` bpm, `f` beat\_phase, `i` beat\_count, `i` bar | Authoritative tempo and position; beat\_phase runs 0 to 1 |
| `/soba/transport` | `i` playing | 1 while the patch plays, 0 when stopped |
| `/soba/hello` | `s` version | Sent once a second; Seoul treats the link as live while messages arrive within 500 ms |

The spec lives in `Seoul/docs/LINK.md`, since Seoul is the side that consumes it. Seoul ignores unknown addresses, so the protocol can grow without breaking older builds.

## Soba: the Link Out module

One new module, `src/modules/link_out.rs`, registered like any other.

- **Ports:** 8 Control inputs (`cv1`…`cv8`), 4 Gate inputs (`gate1`…`gate4`), and a Clock input. The patch decides what Seoul sees by what it plugs in.
- **Audio thread:** `process()` writes the latest value of each lane into an `rtrb` ring, once per block. It never allocates or touches a socket.
- **Sender thread:** an app-side thread drains the ring, keeps the newest value per lane, and sends one bundle every 5 ms. It owns the `UdpSocket` and sends nothing when no Link Out module exists.
- **Panel:** a port field and a small live indicator per lane, matching the look of Soba's other module displays.
- **Desktop only at first.** `cfg(not(target_arch = "wasm32"))` keeps the browser build unchanged.

## Seoul: the link receiver

A new `src/audio/link.rs` runs beside `capture.rs`. Its values join `AudioFeatures` before the triple buffer, so the render thread sees one coherent snapshot, as it does today.

- **Receiver thread:** binds the port, parses bundles, and holds the newest value per lane plus a last-seen time.
- **New features:** `cv: [f32; 8]`, `gate: [f32; 4]` and `link_live`. They also go into the WGSL `AudioFeatures` in `shaders/common.wgsl`, packed as `vec4<f32>` to keep the uniform layout aligned.
- **Expressions:** `preset/expr.rs` gains `cv1`…`cv8` and `gate1`…`gate4`. A preset maps them with expressions it can already write, such as `zoom = "1.0 + cv1 * 0.1"`. No new mapping system is needed.
- **Clock override:** while `link_live` is set, Soba's bpm, beat\_phase and beat\_count replace the detected ones, and `bpm_confidence` reads 1.0. When the link drops, detection takes over again.
- **Latency alignment:** UDP arrives in under 1 ms, but loopback audio lags by tens of milliseconds. A `link_delay_ms` setting in `seoul.toml` delays link values, so a gate flash lands with the kick you hear. It is tuned by ear and hot-reloads like the other settings.
- **Testing:** `--synth` gains a matching fake link, so the existing `--tour` screenshots can check link-driven presets without Soba running.

## Alternatives considered

| Option | Why not |
| --- | --- |
| Visuals inside Soba | Pulls wgpu and Seoul's render passes into an egui app with a browser build. Soba's scope doubles. |
| A synth inside Seoul | Seoul is a listener. Adding a patch editor breaks its clean capture → analysis → render thread model. |
| A third "rig" app | A third thing to maintain and release. It adds nothing two apps and a protocol can't do. |
| A shared Rust crate first | Premature with two consumers. The OSC addresses fit on one page. Extract a crate when a third app (such as a Flint game) joins. |
| MIDI CC instead of OSC | 7-bit values are too coarse for smooth visuals, there is no tempo-plus-phase message, and Windows needs a loopMIDI driver. |

## Milestones

1. **Seoul listens.** Add `link.rs`, the new features and expression variables, and `LINK.md`. Test with any OSC sender, such as a 20-line script. Done when a preset follows `cv1` from the script.
2. **Soba speaks.** Add the Link Out module and sender thread. Done when an LFO patched into `cv1` moves that preset.
3. **Clock lock and alignment.** Add the clock override and `link_delay_ms`. Done when a Soba kick and a gate-driven flash look simultaneous by eye.
4. **Presets that use it.** Write two or three Seoul presets built for the link, plus a matching example patch in Soba's library.
5. **Later.** Seoul-to-Soba messages (preset changes from a sequencer), browser Soba over WebSocket, and Flint games listening.

## Risks

| Risk | Mitigation |
| --- | --- |
| Loopback latency varies by device and buffer size | `link_delay_ms` is user-tunable; consider measuring it with a calibration click later |
| Uniform layout drift between Rust and WGSL | Pack new fields as `vec4`; add a size assertion test beside the struct |
| Windows firewall prompts on first bind | Bind to `127.0.0.1` only, never `0.0.0.0` |
| Presets come to rely on a link that isn't there | Link vars read 0 when the link is down; link presets declare a fallback using the audio features |

## Open questions

- [ ] What range do Soba Control signals use? Should Link Out normalize to 0–1 or send raw values?
- [ ] Are 8 CV and 4 gate lanes enough, or should the lane count be a module setting?
- [ ] Should there be "scenes", where a Soba patch names a Seoul preset to load, or the reverse?
- [ ] Should Seoul's HUD show link status and live lane values (on F1 stats)?
- [ ] Does the Link Out module belong in Soba's default module palette, or behind a setting?
