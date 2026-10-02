SEOUL  ·  a music visualizer
============================

Seoul listens to whatever your Mac is playing and paints it:
bass, beats and melody become light that folds back into itself,
frame after frame, in the MilkDrop tradition.

  This Mac build is UNTESTED. It's built automatically, but nobody
  has run it on a real Mac yet. If something breaks, an issue at
  github.com/chrischaps/Seoul/issues would be very welcome.


GETTING STARTED

  Seoul needs macOS 14.6 (Sonoma) or newer.

  1. Drag Seoul.app into Applications (or anywhere you like).
  2. Open it. The first time, macOS will refuse, because the app
     isn't signed by an Apple-registered developer:
       - Click "Done" on the warning.
       - Open System Settings > Privacy & Security, scroll down,
         and click "Open Anyway" next to Seoul.
     Or, in Terminal:
       xattr -dr com.apple.quarantine /Applications/Seoul.app
  3. When Seoul asks to record system audio, allow it. It only
     listens; the audio never leaves your computer. If you said
     no, turn it on under System Settings > Privacy & Security >
     Screen & System Audio Recording.
  4. Play some music, in any app.

  No music handy? In Terminal:
    /Applications/Seoul.app/Contents/MacOS/seoul --synth


KEYS

  Space / Backspace   next / previous preset
  R                   shuffle
  A                   auto-advance on the beat
  L                   lock the current preset
  F                   favorite      1-9  jump to a favorite
  X                   hide a preset you don't like
  Ctrl-Cmd-F          fullscreen (or the green window button)
  H                   help          F1   stats
  P                   screenshot
  Esc / Cmd-Q         quit


MAKE IT YOURS

  On first launch Seoul copies its presets and settings to

    ~/Library/Application Support/Seoul

  (in Finder: Go > Go to Folder...). Everything lives there:

  - seoul.toml holds the settings: auto-advance timing,
    transition styles, bloom, grain, and more. Edits apply while
    Seoul is running.
  - Each preset in presets/ is a .toml file plus a .wgsl shader.
    Edit one while it's on screen and it reloads instantly. Copy
    one to start your own.
  - Screenshots are saved to screenshots/.

  Your edits are kept when you update Seoul.


  https://chaps.dev/projects/seoul
  https://github.com/chrischaps/Seoul
