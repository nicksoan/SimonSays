# SimonSays

A small desktop macro recorder, similar to TinyTask. Record mouse movement,
clicks, scrolling, and keyboard presses, then replay the sequence with its
original timing, once or repeatedly.

## Setup

Requires Python 3.10+ and a graphical desktop.

```sh
python -m venv .venv
# Windows:
.venv\Scripts\activate
# macOS / Linux:
source .venv/bin/activate
python -m pip install -r requirements.txt
python simonsays.py
```

- **Windows:** use a standard Python installation with Tcl/Tk support. Elevated
  applications may require running SimonSays with matching permissions.
- **macOS:** grant Accessibility and Input Monitoring permissions to the terminal
  or Python application in System Settings.
- **Linux:** requires an X11 session and Tkinter (on Debian/Ubuntu,
  `sudo apt install python3-tk`). Global capture is not supported on a pure
  Wayland desktop.

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

The core tests use fake input controllers and need no desktop or dependencies:

```sh
python -m unittest discover -s tests -v
```