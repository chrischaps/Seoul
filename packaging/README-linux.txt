SEOUL  ·  a music visualizer
============================

Seoul listens to the room, or to a line-in, and paints it: bass,
beats and melody become light that folds back into itself, frame
after frame, in the MilkDrop tradition.


GETTING STARTED

  1. Keep this folder together. seoul needs the presets folder
     beside it.
  2. Plug in a microphone, or run  ./seoul --list-inputs  to see
     what's there.
  3. Run  ./seoul --mic

  Linux has no system-audio loopback, so Seoul listens to inputs.
  Under PulseAudio or PipeWire, a "monitor" input carries what the
  computer is playing:  ./seoul --input monitor

  No music handy? Run  ./seoul --synth  for a built-in test track.

  ./seoul --frame --mic  makes it a picture on the wall: fullscreen,
  resting when the room is quiet, playing when a record goes on,
  asleep from 11 pm to 7 am. [frame] in seoul.toml sets the hours.


KEYS

  Space / Backspace   next / previous preset
  R                   shuffle
  A                   auto-advance on the beat
  L                   lock the current preset
  F                   favorite      1-9  jump to a favorite
  X                   hide a preset you don't like
  F11                 fullscreen
  H                   help          F1   stats
  P                   screenshot (saved to screenshots/)
  Esc                 quit

  On a touchscreen: tap for the next preset, tap with two fingers
  to quit.


ON A RASPBERRY PI 5

  The aarch64 build runs on a Pi 5 with Raspberry Pi OS. Draw the
  feedback at half resolution: on the 7" Touch Display 2, about
  half the presets hold 60 fps that way, and none do at full size.

    ./seoul --frame --mic --render-scale 0.5

  or set  render_scale = 0.5  in seoul.toml.


RELEASES AND CHECKSUMS

  Every release carries SHA256SUMS. To check a download:

    sha256sum -c SHA256SUMS --ignore-missing

  Easel, the frame's launcher, installs Seoul from these releases
  and checks the same file. It sees only published releases:
  GitHub hides drafts from its API, so a tagged build reaches the
  frame only when someone writes its notes and publishes it.


REQUIREMENTS

  64-bit Linux (x86_64 or aarch64) with glibc 2.36 or newer
  (Debian 12, Ubuntu 22.10, Raspberry Pi OS bookworm), ALSA, and a
  Vulkan or OpenGL ES 3 GPU. Wayland or X11. The HUD's fonts:

    sudo apt install fonts-noto-core fonts-noto-cjk fonts-dejavu-core


  https://chaps.dev/projects/seoul
  https://github.com/chrischaps/Seoul
