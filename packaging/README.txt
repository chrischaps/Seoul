SEOUL  ·  a music visualizer
============================

Seoul listens to whatever your computer is playing and paints it:
bass, beats and melody become light that folds back into itself,
frame after frame, in the MilkDrop tradition.


GETTING STARTED

  1. Keep this folder together. seoul.exe needs the presets folder
     beside it.
  2. Play some music, in any app.
  3. Double-click seoul.exe.

  Windows may say "Windows protected your PC" the first time. That
  appears because the app isn't code-signed. Click "More info", then
  "Run anyway".

  No music handy? Run  seoul.exe --synth  for a built-in test track.


KEYS

  Space / Backspace   next / previous preset
  R                   shuffle
  A                   auto-advance on the beat
  L                   lock the current preset
  F                   favorite      1-9  jump to a favorite
  X                   hide a preset you don't like
  F11                 fullscreen
  H                   help          F1   stats
  P                   screenshot (saved to screenshots\)
  Esc                 quit


MAKE IT YOURS

  seoul.toml holds the settings: auto-advance timing, transition
  styles, bloom, grain, and more. Edits apply while Seoul is running.

  Each preset is a .toml file plus a .wgsl shader in presets\. Edit
  one while it's on screen and it reloads instantly. Copy one to
  start your own.

  seoul.exe --help lists the command-line options.


REQUIREMENTS

  Windows 10 or 11, 64-bit, with a DirectX 12 or Vulkan GPU.


  https://chaps.dev/projects/seoul
  https://github.com/chrischaps/Seoul
