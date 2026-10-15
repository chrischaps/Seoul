# The Soba link

A Soba patch can play Seoul directly. Soba's **Link Out** module sends its
control lanes, gates and clock to Seoul over OSC, and Seoul hands them to
presets as expression variables and shader fields. Audio still reaches
Seoul through loopback, exactly as before. Only control data travels the
wire.

Neither app depends on the other. This page is the whole contract: any OSC
sender that follows it (TouchDesigner, VCV Rack, a 20-line script) can play
Seoul the same way.

## Transport

- **OSC 1.0 over UDP to `127.0.0.1`, port 9137.** Seoul binds the loopback
  interface only, so there is no firewall prompt and no remote sender.
  Change the port with `[link] port` in `seoul.toml` or `--link-port`.
- The sender sends **one bundle per tick at about 200 Hz** (every 5 ms),
  and every bundle carries every lane, so the next one repairs a dropped
  packet. Bare messages work too. Seoul reads bundle contents
  immediately and ignores timetags.
- The link is **live** while messages arrive within 500 ms of each other.

## Addresses

| Address | Arguments | Meaning |
| --- | --- | --- |
| `/soba/cv/1` … `/soba/cv/8` | `f` value | A control signal, sent as Soba has it: 0..1 unipolar or −1..1 bipolar. Seoul never clamps. |
| `/soba/gate/1` … `/soba/gate/4` | `i` 0 or 1 | A gate's state. Seoul counts a rising edge as a trigger. |
| `/soba/clock` | `f` bpm, `f` beat_phase, `i` beat_count, `i` bar | Authoritative tempo and position. beat_phase runs 0→1 within a beat. beat_count and bar count up from the start of play. |
| `/soba/transport` | `i` playing | 1 while the patch plays, 0 when stopped. Until one arrives, Seoul assumes playing. |
| `/soba/hello` | `s` version | Sent once a second. Seoul logs it and shows it in the F1 panel. |

Seoul accepts any numeric OSC type (`f`, `d`, `i`, `h`, `T`/`F`) wherever a
number is expected. **Unknown addresses and malformed arguments are
ignored**, so the protocol can grow without breaking older builds.

## What presets see

| Expression | WGSL (`u.…`) | Meaning |
| --- | --- | --- |
| `cv1` … `cv8` | `cv[0]` … `cv[7]` | The control lanes |
| `gate1` … `gate4` | `gate[0]` … `gate[3]` | Gate state, 0 or 1 |
| `gate1_age` … | `gate_age[i]` | Seconds since that gate's last rising edge (capped at 1000) |
| `gate1_count` … | `gate_count[i]` | Rising edges since the link came up |
| `link_live` | `link_live` | 1 while the link is live |
| `playing` | `playing` | 1 while the linked patch plays |
| `bar` | `bar` | Soba's bar while the link plays, else `floor(beat_count / 4)` |

**When the link is down, every lane reads 0 and every gate age reads 1000**,
so age-driven flashes stay dark. A link preset should still look good
without Soba. The idiom is to blend toward an audio feature:

```toml
zoom = "1.0 + mix(bass_att, cv1, link_live) * 0.1"
```

In WGSL, do the same with `mix(heard, u.cv[0], u.link_live)`, or branch on
`u.link_live > 0.5` to pick `u.gate_age[g]` or a drum's `kick_age`.
Gayageum and Obangsaek are worked examples.

### Gates latch

A 5 ms trigger can fall between two 60 fps frames. Seoul counts rising
edges on the receiver thread, at the full 200 Hz, so a gate is never
missed. Draw triggers from `gateN_age` (or `gateN_count`), as you would
with `kick_age`, rather than from `gateN`.

### Clock lock

While the link is live **and playing** and a `/soba/clock` arrived in the
last 500 ms, Soba's clock replaces tempo detection:

- `bpm` is Soba's tempo, and `bpm_confidence` reads 1.0 (the F1 panel says
  `conf link`).
- `beat_phase` is extrapolated from the newest clock message at Soba's
  tempo, so it moves smoothly every frame. `beat_count` steps when the
  phase wraps.
- `bar` is Soba's bar.

Every preset that follows `beat_phase` then locks to the patch, as does
auto-advance's downbeat wait. A stopped transport hands the clock back to
detection, because a frozen phase would freeze those presets. When the
link drops, detection takes over again with one jump in phase.

## Latency: `delay_ms`

UDP arrives in well under a millisecond, but loopback audio reaches Seoul
tens of milliseconds after it plays. `[link] delay_ms` shifts the whole
link into the past (values, gate edges and clock together), so a gate flash
lands with the kick you hear. Tune it by ear. It hot-reloads and ranges
over 0–1000 ms.

```toml
[link]
enabled = true
port = 9137
delay_ms = 40
```

## How Seoul does it

`src/audio/link.rs`: a `seoul-link` thread receives packets, folds each
message into a `LinkState`, counts gate edges, and sends a timestamped
snapshot per packet to the render thread. The render thread keeps two
seconds of snapshots and, each frame, merges the one from `delay_ms` ago
into `AudioFeatures`, after analysis and before the timing stamp. The merge
lives on the render thread for two reasons: `delay_ms` is measured against
the clock that presents the frame, and `--record` runs analysis on that
thread too.

If the port is taken (a second Seoul, say), the link reports `port busy` in
the F1 panel, and Seoul carries on without it.

## Trying it without Soba

- **`--synth`** starts a stand-in Soba (`src/audio/link_fake.rs`) that plays
  along with the test track over real UDP: an LFO swell, a bipolar bar LFO,
  a bar ramp, a breakdown flag and more on the CV lanes, and kick, clap,
  hats and the downbeat on the gates. `--no-link` turns both the stand-in
  and the listener off, which shows a preset's fallback.
- **`--record`** folds the same stand-in in directly, at 200 Hz on the
  recording's clock, so recordings stay deterministic.
- **`cargo run --release --example link_send [PORT]`** sends the stand-in
  lanes alone, so you can play link presets against real music through
  loopback.
- **F1** shows link status, eight CV columns and four gate lamps.
