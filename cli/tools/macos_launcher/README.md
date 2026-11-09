# macOS `--compress` launcher

`launcher` is the executable of a macOS app built with
`ende desktop --compress`. The thin bundle holds it as `Contents/MacOS/<App>`
next to the compressed real bundle in `Contents/Resources/`. On launch it
extracts the real bundle to `~/Library/Application Support/<bundle id>/<hash>/`
(once; later launches reuse it) and opens it with LaunchServices
(`/usr/bin/open <App>.app --args ...`).

Why a compiled launcher that opens the app, and not a script that execs it:

- LaunchServices registers the process it launched. After an `exec` that
  registration no longer matches the process, and the app's `NSStatusItem`s
  (`Ende.Tray`) never reach the menu bar (denoland/deno#35619). This happens
  with a compiled launcher that calls `execv` too, not only with a script.
- Opened through LaunchServices, the real bundle runs as an app of its own: its
  tray icons show, it has one Dock icon, and opening the thin bundle again
  brings a running copy to the front instead of starting a second one.
- A bundle's main executable should be Mach-O for signing and notarization.

What follows from opening the app instead of exec'ing it: the launcher exits as
soon as LaunchServices has started the app, the app doesn't inherit the
launcher's environment, standard streams or exit status, and arguments reach it
only when it isn't already running (as with `open --args`).

It's C, using only CoreFoundation and libc, so the universal binary for both
macOS targets is about 85 KB. It's vendored, like the AppImage runtimes, so
`ende` can build `--compress` apps for macOS on any host without an Apple
toolchain.

| File       | SHA-256                                                          |
| ---------- | ---------------------------------------------------------------- |
| launcher.c | 9df3dbc985f4ca568562f6a17906ba3a421ba379cbe667c29f61d519339612fe |
| launcher   | ae2ee9eef9e5d492f8f18310bd48fd1e0fe9833257a45d96d9e183fca3a6be2c |

Built with Apple clang 21.0.0 (clang-2100.3.34.2), macOS 10.15 minimum, arm64 +
x86_64. The build is reproducible with the same clang.

To change it, edit launcher.c, run `./build.sh` on macOS, and update the
SHA-256s here and in `cli/build.rs` (`MACOS_LAUNCHER_HASHES`). The build fails
while either file doesn't match its pin.
