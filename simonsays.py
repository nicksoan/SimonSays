"""SimonSays desktop application. Run with: python simonsays.py."""

import sys
from queue import Empty, SimpleQueue
from threading import local

from macro import Macro


def main():
    try:
        import tkinter as tk
        from tkinter import ttk

        from pynput import keyboard, mouse

    except (ImportError, OSError, RuntimeError) as exc:
        print(f"Cannot start SimonSays: {exc}", file=sys.stderr)
        print("See README.md for desktop requirements and setup.", file=sys.stderr)
        return 1

    try:
        root = tk.Tk()
        mouse_controller = mouse.Controller()
        keyboard_controller = keyboard.Controller()
    except (tk.TclError, OSError, RuntimeError) as exc:
        print(f"Cannot open desktop: {exc}", file=sys.stderr)
        return 1

    root.title("SimonSays")
    root.resizable(False, False)
    macro = Macro(mouse_controller, keyboard_controller)
    commands = SimpleQueue()
    shortcuts_down = set()
    key_event = local()
    shortcuts = {
        keyboard.Key.f8: "record",
        keyboard.Key.f9: "play",
        keyboard.Key.esc: "stop",
    }

    def key_press(key):
        if key in shortcuts:
            if key not in shortcuts_down:
                shortcuts_down.add(key)
                if key == keyboard.Key.esc:
                    macro.stop()
                commands.put(shortcuts[key])
            return
        macro.capture("key", key, True, key_id=getattr(key_event, "code", None))

    def key_release(key):
        if key in shortcuts:
            shortcuts_down.discard(key)
            return
        macro.capture("key", key, False, key_id=getattr(key_event, "code", None))

    def xorg_event_filter(event):
        # X11 keysyms change with modifiers; event.detail is the physical keycode.
        key_event.code = event.detail
        return True

    listeners = [
        mouse.Listener(
            on_move=lambda x, y: macro.capture("move", x, y),
            on_click=lambda x, y, button, pressed: macro.capture(
                "click", x, y, button, pressed
            ),
            on_scroll=lambda x, y, dx, dy: macro.capture("scroll", x, y, dx, dy),
        ),
        keyboard.Listener(
            on_press=key_press,
            on_release=key_release,
            xorg_event_filter=xorg_event_filter,
        ),
    ]

    frame = ttk.Frame(root, padding=16)
    frame.grid()
    ttk.Label(frame, text="SimonSays", font=("", 18, "bold")).grid(
        row=0, column=0, columnspan=3, pady=(0, 8)
    )
    ttk.Label(
        frame, text="F8: record / stop    F9: play    Esc: stop"
    ).grid(row=1, column=0, columnspan=3)
    status = tk.StringVar(value="Ready — record a short sequence.")
    notice = ""
    repeats = tk.StringVar(value="1")

    def record():
        nonlocal notice
        if macro.status()[0] == "recording":
            macro.stop()
        elif macro.record():
            notice = ""
            root.iconify()

    def play():
        nonlocal notice
        try:
            count = int(repeats.get())
            if count < 1:
                raise ValueError
        except ValueError:
            notice = "Repeat count must be a positive integer."
            return
        if macro.play(count):
            notice = ""
            root.iconify()
        elif macro.status()[0] == "idle":
            notice = "Record some actions first."

    record_button = ttk.Button(frame, text="Record (F8)", command=record)
    record_button.grid(row=2, column=0, padx=4, pady=12)
    stop_button = ttk.Button(frame, text="Stop (Esc)", command=macro.stop)
    stop_button.grid(row=2, column=1, padx=4)
    play_button = ttk.Button(frame, text="Play (F9)", command=play)
    play_button.grid(row=2, column=2, padx=4)
    ttk.Label(frame, text="Repeat count:").grid(row=3, column=0)
    repeat_input = ttk.Spinbox(
        frame, from_=1, to=999, textvariable=repeats, width=8
    )
    repeat_input.grid(row=3, column=1)
    ttk.Label(frame, textvariable=status, wraplength=360).grid(
        row=4, column=0, columnspan=3, pady=(12, 0)
    )
    previous_mode = "idle"

    def poll():
        nonlocal previous_mode
        try:
            while True:
                command = commands.get_nowait()
                if command == "record":
                    record()
                elif command == "play":
                    play()
                elif command == "stop":
                    macro.stop()
        except Empty:
            pass
        mode, count, error = macro.status()
        if mode == "idle" and previous_mode != "idle":
            root.deiconify()
        previous_mode = mode
        record_button.configure(state="disabled" if mode == "playing" else "normal")
        play_button.configure(state="normal" if mode == "idle" else "disabled")
        stop_button.configure(state="disabled" if mode == "idle" else "normal")
        repeat_input.configure(state="normal" if mode == "idle" else "disabled")
        status.set(
            error or notice or {
                "idle": f"Ready — {count} steps recorded.",
                "recording": f"Recording — {count} steps. Press F8 or Esc to stop.",
                "playing": "Playing — starts after 1 second. Press Esc to stop.",
            }[mode]
        )
        if not all(listener.is_alive() for listener in listeners):
            macro.stop()
            status.set("Input listener stopped. Restart SimonSays; check permissions.")
            record_button.configure(state="disabled")
            play_button.configure(state="disabled")
            return
        root.after(50, poll)

    def close():
        macro.stop()
        for listener in listeners:
            listener.stop()
        macro.wait(2)
        root.destroy()

    root.protocol("WM_DELETE_WINDOW", close)
    try:
        for listener in listeners:
            listener.start()
            listener.wait()
        root.after(50, poll)
        root.mainloop()
    except (OSError, RuntimeError) as exc:
        print(f"Input listener failed: {exc}", file=sys.stderr)
        close()
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
