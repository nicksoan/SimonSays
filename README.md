# SimonSays

A small Rust desktop macro recorder, similar to TinyTask. Record mouse movement,
clicks, scrolling, and keyboard presses, then replay the sequence with its
original timing, once or repeatedly.

## Setup

Requires Rust 1.95 or newer (install from https://rustup.rs), a
platform C/C++ linker, and a graphical desktop. No Python runtime is needed.

```sh
cargo run --release --locked
```

- **Windows:** install the MSVC build tools when prompted by rustup. Elevated
  applications may require running SimonSays with matching permissions.
- **macOS:** grant Accessibility and Input Monitoring permissions to the terminal
  or SimonSays application in System Settings. Missing permissions can silently
  prevent global recording; restart the application after granting them.
- **Linux:** requires an X11 session and development libraries. On Debian/Ubuntu:
  `sudo apt install build-essential pkg-config libx11-dev libxi-dev libxtst-dev libgl1-mesa-dev libxkbcommon-x11-0`.
  Global capture is not supported on a pure Wayland desktop.

To build without launching, run `cargo build --release --locked`. The executable
is `target/release/simonsays` (`simonsays.exe` on Windows). Subsequent launches of
this executable do not require Cargo or the Rust toolchain.

## Usage

1. Click **Record** or press **F8**. The window minimizes; perform your actions.
2. Press **F8** or **Esc** to stop recording. The window returns.
3. Choose a positive **Repeat count**, then click **Play** or press **F9**.
   Playback starts after one second so you can focus the target application.
4. Press **Esc** at any time to stop. Playback releases inputs it is holding.

F8, F9, and Esc are reserved controls and are not recorded. A new recording
replaces the previous one. Macros stay in memory only and are lost on exit;
no input is saved to disk or sent over the network. Only record trusted actions:
playback uses absolute screen coordinates, so keep windows and display layout
unchanged. Avoid typing passwords or other sensitive data while recording.

## Tests

The regression tests use fake input simulation and need no running desktop.
Linux still needs the development libraries above to compile.

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```