import unittest
from threading import Event
from unittest.mock import Mock, patch

from macro import Macro


class Controller:
    def __init__(self):
        self.calls = []
        self.pressed = Event()

    @property
    def position(self):
        return self._position

    @position.setter
    def position(self, value):
        self._position = value
        self.calls.append(("move", value))

    def press(self, value):
        self.calls.append(("press", value))
        self.pressed.set()

    def release(self, value):
        self.calls.append(("release", value))

    def scroll(self, dx, dy):
        self.calls.append(("scroll", dx, dy))


class MacroTests(unittest.TestCase):
    def setUp(self):
        self.mouse = Controller()
        self.keyboard = Controller()
        self.now = 0.0
        self.macro = Macro(self.mouse, self.keyboard, clock=lambda: self.now)

    def test_records_and_replays_all_input_types_in_order(self):
        self.macro.capture("key", "ignored", True)
        self.assertTrue(self.macro.record())
        self.macro.capture("move", 10, 20)
        self.macro.capture("click", 10, 20, "left", True)
        self.macro.capture("click", 10, 20, "left", False)
        self.macro.capture("scroll", 30, 40, 0, -1)
        self.macro.capture("key", "a", True)
        self.macro.capture("key", "a", False)
        self.macro.stop()
        self.assertTrue(self.macro.play(repeats=2, delay=0))
        self.macro.wait(1)
        self.assertEqual(self.macro.status()[0], "idle")
        self.assertEqual(self.keyboard.calls, [
            ("press", "a"), ("release", "a"),
            ("press", "a"), ("release", "a"),
        ])
        self.assertEqual(self.mouse.calls, [
            ("move", (10, 20)), ("move", (10, 20)), ("press", "left"),
            ("move", (10, 20)), ("release", "left"),
            ("move", (30, 40)), ("scroll", 0, -1),
        ] * 2)

    def test_exclusive_modes_and_replacement(self):
        self.assertFalse(self.macro.play(delay=0))
        self.macro.record()
        self.assertFalse(self.macro.record())
        self.assertFalse(self.macro.play(delay=0))
        self.macro.capture("move", 1, 2)
        self.macro.stop()
        self.assertTrue(self.macro.play(delay=10))
        self.assertFalse(self.macro.record())
        self.assertFalse(self.macro.play())
        self.macro.capture("move", 3, 4)
        self.macro.stop()
        self.macro.wait(1)
        self.assertEqual(self.mouse.calls, [])
        self.assertTrue(self.macro.record())
        self.assertEqual(self.macro.status()[1], 0)

    def test_stop_interrupts_wait_and_releases_held_inputs(self):
        self.macro.record()
        self.macro.capture("key", "shift", True)
        self.now = 60
        self.macro.capture("key", "shift", False)
        self.macro.stop()
        self.assertTrue(self.macro.play(delay=0))
        self.assertTrue(self.keyboard.pressed.wait(1))
        self.macro.stop()
        self.macro.wait(1)
        self.assertEqual(self.macro.status()[0], "idle")
        self.assertEqual(self.keyboard.calls, [
            ("press", "shift"), ("release", "shift"),
        ])

    def test_ignores_unmatched_releases_and_balances_recording(self):
        self.macro.record()
        self.macro.capture("key", "a", False)
        self.macro.capture("click", 1, 2, "left", False)
        self.macro.capture("key", "shift", True)
        self.macro.capture("click", 1, 2, "left", True)
        self.macro.stop()
        self.macro.play(delay=0)
        self.macro.wait(1)
        self.assertEqual(self.keyboard.calls, [
            ("press", "shift"), ("release", "shift"),
        ])
        self.assertEqual(self.mouse.calls[-1], ("release", "left"))

    def test_playback_error_restores_idle_and_releases_keys(self):
        def fail(value):
            raise RuntimeError("controller unavailable")

        self.macro.record()
        self.macro.capture("key", "a", True)
        self.macro.stop()
        self.keyboard.press = fail
        self.macro.play(delay=0)
        self.macro.wait(1)
        self.assertEqual(self.macro.status()[0], "idle")
        self.assertIn("controller unavailable", self.macro.status()[2])
        self.assertEqual(self.keyboard.calls, [("release", "a")])

    def test_empty_recording_and_invalid_repeat_count(self):
        self.macro.record()
        self.macro.stop()
        self.assertFalse(self.macro.play(delay=0))
        for count in (0, -1, 1.5, "2"):
            with self.assertRaises(ValueError):
                self.macro.play(repeats=count)

    def test_preserves_timing_and_final_pause_on_each_repeat(self):
        self.macro.record()
        self.now = 0.25
        self.macro.capture("move", 1, 2)
        self.now = 1
        self.macro.stop()
        waits = []

        def advance_clock(timeout):
            waits.append(timeout)
            self.now += timeout
            return False

        with patch.object(self.macro._stop, "wait", side_effect=advance_clock):
            self.macro.play(repeats=2, delay=0)
            self.macro.wait(1)
        self.assertEqual(waits, [0, 0.25, 0.75, 0.25, 0.75])

    def test_matches_key_release_after_modifier_changes(self):
        uppercase = Mock(vk=65, char="A")
        lowercase = Mock(vk=65, char="a")
        self.macro.record()
        self.macro.capture("key", uppercase, True)
        self.macro.capture("key", lowercase, False)
        self.macro.stop()
        self.macro.play(delay=0)
        self.macro.wait(1)
        self.assertEqual(self.keyboard.calls, [
            ("press", uppercase), ("release", uppercase),
        ])

    def test_matches_shifted_punctuation_using_native_keycode(self):
        shifted = Mock(vk=33, char="!")
        unshifted = Mock(vk=49, char="1")
        self.macro.record()
        self.macro.capture("key", shifted, True, key_id=10)
        self.macro.capture("key", unshifted, False, key_id=10)
        self.macro.stop()
        self.macro.play(delay=0)
        self.macro.wait(1)
        self.assertEqual(self.keyboard.calls, [
            ("press", shifted), ("release", shifted),
        ])


if __name__ == "__main__":
    unittest.main()
