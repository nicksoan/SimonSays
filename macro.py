"""Thread-safe, in-memory macro recording and timed playback."""

from dataclasses import dataclass
from threading import Event, Lock, Thread
from time import monotonic


@dataclass(frozen=True)
class Step:
    offset: float
    action: str
    args: tuple


class Macro:
    def __init__(self, mouse, keyboard, clock=monotonic):
        self.mouse = mouse
        self.keyboard = keyboard
        self.clock = clock
        self._lock = Lock()
        self._stop = Event()
        self._steps = []
        self._mode = "idle"
        self._error = ""
        self._worker = None
        self._held = {}

    def status(self):
        with self._lock:
            return self._mode, len(self._steps), self._error

    def record(self):
        with self._lock:
            if self._mode != "idle":
                return False
            self._steps = []
            self._held = {}
            self._error = ""
            self._started = self.clock()
            self._mode = "recording"
            return True

    def capture(self, action, *args, key_id=None):
        with self._lock:
            if self._mode != "recording":
                return
            if action in ("key", "click"):
                value = args[-2]
                vk = getattr(value, "vk", None)
                physical = key_id if key_id is not None else vk
                identity = (
                    action,
                    physical if action == "key" and physical is not None else value,
                )
                if args[-1]:
                    value = self._held.setdefault(identity, value)
                elif identity not in self._held:
                    return
                else:
                    value = self._held.pop(identity)
                if action == "key":
                    args = (value, args[-1])
            self._steps.append(Step(self.clock() - self._started, action, args))

    def stop(self):
        with self._lock:
            if self._mode == "recording":
                # Preserve the final pause and balance inputs held when stopped.
                offset = self.clock() - self._started
                for (action, _), value in self._held.items():
                    if action == "key":
                        self._steps.append(Step(offset, "key", (value, False)))
                    else:
                        self._steps.append(Step(offset, "button_release", (value,)))
                self._held.clear()
                self._steps.append(Step(offset, "pause", ()))
                self._mode = "idle"
            elif self._mode == "playing":
                self._stop.set()

    def play(self, repeats=1, delay=1.0):
        if not isinstance(repeats, int) or repeats < 1:
            raise ValueError("Repeat count must be a positive integer")
        with self._lock:
            if self._mode != "idle" or not any(
                step.action != "pause" for step in self._steps
            ):
                return False
            steps = tuple(self._steps)
            self._stop.clear()
            self._error = ""
            self._mode = "playing"
            self._worker = Thread(
                target=self._play, args=(steps, repeats, delay), daemon=True
            )
            self._worker.start()
            return True

    def wait(self, timeout=None):
        with self._lock:
            worker = self._worker
        if worker:
            worker.join(timeout)

    def _play(self, steps, repeats, delay):
        held = {}
        try:
            if self._stop.wait(max(0, delay)):
                return
            for _ in range(repeats):
                started = self.clock()
                for step in steps:
                    remaining = max(0, started + step.offset - self.clock())
                    if self._stop.wait(remaining):
                        return
                    action, args = step.action, step.args
                    if action in ("move", "click", "scroll"):
                        self.mouse.position = args[:2]
                    if action == "scroll":
                        self.mouse.scroll(*args[2:])
                    elif action in ("key", "click", "button_release"):
                        controller = self.keyboard if action == "key" else self.mouse
                        value = args[0] if action != "click" else args[2]
                        pressed = args[-1] if action != "button_release" else False
                        identity = (action == "key", value)
                        if pressed:
                            held[identity] = (controller, value)
                            controller.press(value)
                        else:
                            controller.release(value)
                            held.pop(identity, None)
                self._release(held)
        except Exception as exc:
            with self._lock:
                self._error = f"Playback failed: {exc}"
        finally:
            self._release(held)
            with self._lock:
                self._mode = "idle"

    def _release(self, held):
        for controller, value in reversed(list(held.values())):
            try:
                controller.release(value)
            except Exception as exc:
                with self._lock:
                    self._error = f"Could not release input: {exc}"
        held.clear()
