use rdev::{Button, EventType, Key};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Idle,
    Recording,
    Playing,
}

pub struct Snapshot {
    pub mode: Mode,
    pub count: usize,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct Recorder {
    state: Arc<Mutex<State>>,
    simulator: Arc<Simulator>,
    focus_delay: Duration,
}

type Simulator = dyn Fn(&EventType) -> Result<(), String> + Send + Sync;
type Cancellation = Arc<(Mutex<bool>, Condvar)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    Key(Key),
    Button(Button),
}

impl Held {
    fn release(self) -> EventType {
        match self {
            Self::Key(key) => EventType::KeyRelease(key),
            Self::Button(button) => EventType::ButtonRelease(button),
        }
    }
}

#[derive(Clone, Debug)]
struct RecordedEvent {
    offset: Duration,
    event: EventType,
    position: Option<(f64, f64)>,
}

struct State {
    mode: Mode,
    events: Vec<RecordedEvent>,
    started: Option<Instant>,
    duration: Duration,
    held: Vec<Held>,
    mouse: Option<(f64, f64)>,
    controls: Vec<Key>,
    error: Option<String>,
    cancellation: Option<Cancellation>,
    playback: Option<JoinHandle<()>>,
    echoes: VecDeque<(Instant, EventType)>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    pub fn new() -> Self {
        Self::with_simulator(
            Arc::new(|event| rdev::simulate(event).map_err(|error| error.to_string())),
            Duration::from_secs(1),
        )
    }

    fn with_simulator(simulator: Arc<Simulator>, focus_delay: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                mode: Mode::Idle,
                events: Vec::new(),
                started: None,
                duration: Duration::ZERO,
                held: Vec::new(),
                mouse: None,
                controls: Vec::new(),
                error: None,
                cancellation: None,
                playback: None,
                echoes: VecDeque::new(),
            })),
            simulator,
            focus_delay,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let state = lock(&self.state);
        Snapshot {
            mode: state.mode,
            count: state.events.len(),
            error: state.error.clone(),
        }
    }

    pub fn toggle_record(&self) {
        let mut state = lock(&self.state);
        match state.mode {
            Mode::Idle => {
                state.events.clear();
                state.held.clear();
                state.duration = Duration::ZERO;
                state.started = Some(Instant::now());
                state.error = None;
                state.mode = Mode::Recording;
            }
            Mode::Recording => finish_recording(&mut state),
            Mode::Playing => {}
        }
    }

    pub fn play(&self, repeats: u32) -> Result<(), String> {
        let mut state = lock(&self.state);
        let invalid = if state.mode != Mode::Idle {
            Some("Stop recording or playback before playing.")
        } else if repeats == 0 {
            Some("Repeat count must be at least one.")
        } else if state.events.is_empty() {
            Some("Record some input before playing.")
        } else {
            None
        };
        if let Some(message) = invalid {
            state.error = Some(message.into());
            return Err(message.into());
        }
        let events = state.events.clone();
        let duration = state.duration;
        let cancellation = Arc::new((Mutex::new(false), Condvar::new()));
        state.cancellation = Some(cancellation.clone());
        state.mode = Mode::Playing;
        state.error = None;
        let recorder = self.clone();
        // The completed worker has already released every synthetic input before
        // publishing Idle. Joining it is unnecessary when replacing its handle.
        match thread::Builder::new()
            .name("macro-playback".into())
            .spawn(move || {
                let error = recorder.run(&events, duration, repeats, &cancellation);
                let mut state = lock(&recorder.state);
                state.error = error;
                state.cancellation = None;
                state.mode = Mode::Idle;
            }) {
            Ok(handle) => state.playback = Some(handle),
            Err(error) => {
                let message = format!("Could not start playback: {error}");
                state.mode = Mode::Idle;
                state.cancellation = None;
                state.error = Some(message.clone());
                return Err(message);
            }
        }
        Ok(())
    }

    pub fn stop(&self) {
        let mut state = lock(&self.state);
        match state.mode {
            Mode::Recording => finish_recording(&mut state),
            Mode::Playing => {
                if let Some(cancellation) = &state.cancellation {
                    *lock(&cancellation.0) = true;
                    cancellation.1.notify_all();
                }
            }
            Mode::Idle => {}
        }
    }

    pub fn wait(&self) {
        let handle = lock(&self.state).playback.take();
        if let Some(handle) = handle {
            if handle.thread().id() == thread::current().id() {
                // A synchronous simulator callback must not join its own worker.
                lock(&self.state).playback = Some(handle);
            } else if handle.join().is_err() {
                let mut state = lock(&self.state);
                state.error = Some("Playback worker failed unexpectedly.".into());
                state.mode = Mode::Idle;
                state.cancellation = None;
            }
        }
    }

    pub fn handle_input(&self, event: EventType, repeats: u32) {
        let mut state = lock(&self.state);
        match event {
            EventType::KeyPress(key) if is_control(key) => {
                if state.controls.contains(&key) {
                    return;
                }
                state.controls.push(key);
                drop(state);
                match key {
                    Key::F8 => self.toggle_record(),
                    Key::F9 => {
                        let _ = self.play(repeats);
                    }
                    Key::Escape => self.stop(),
                    _ => {}
                }
                return;
            }
            EventType::KeyRelease(key) if is_control(key) => {
                state.controls.retain(|held| *held != key);
                return;
            }
            _ => {}
        }
        // rdev exposes no injected-event identifier. Keep bounded, expiring
        // receipts so delayed listener echoes cannot contaminate a new recording.
        let now = Instant::now();
        while state
            .echoes
            .front()
            .is_some_and(|(expires, _)| *expires <= now)
        {
            state.echoes.pop_front();
        }
        if let Some(index) = state.echoes.iter().position(|(_, echo)| *echo == event) {
            state.echoes.remove(index);
            if let EventType::MouseMove { x, y } = event {
                state.mouse = Some((x, y));
            }
            return;
        }
        if let EventType::MouseMove { x, y } = event {
            state.mouse = Some((x, y));
        }
        if state.mode != Mode::Recording {
            return;
        }
        if !update_held(&mut state.held, event) {
            return;
        }
        let offset = state
            .started
            .map_or(Duration::ZERO, |start| start.elapsed());
        let position = position_for(event, state.mouse);
        state.events.push(RecordedEvent {
            offset,
            event,
            position,
        });
    }

    fn inject(&self, event: EventType) -> Result<(), String> {
        {
            let mut state = lock(&self.state);
            if state.echoes.len() >= 4096 {
                state.echoes.pop_front();
            }
            state
                .echoes
                .push_back((Instant::now() + Duration::from_secs(1), event));
        }
        // Never hold State across an OS call: macOS may invoke the listener here.
        (self.simulator)(&event)
    }

    fn run(
        &self,
        events: &[RecordedEvent],
        duration: Duration,
        repeats: u32,
        cancellation: &Cancellation,
    ) -> Option<String> {
        let mut held = Vec::new();
        let result = (|| -> Result<(), String> {
            let focus = Instant::now()
                .checked_add(self.focus_delay)
                .ok_or("Playback delay is too large.")?;
            if !wait_until(cancellation, focus) {
                return Ok(());
            }
            for _ in 0..repeats {
                let start = Instant::now();
                for recorded in events {
                    let deadline = start
                        .checked_add(recorded.offset)
                        .ok_or("Recording duration is too large.")?;
                    if !wait_until(cancellation, deadline) {
                        return Ok(());
                    }
                    if let Some((x, y)) = recorded.position {
                        self.inject(EventType::MouseMove { x, y })?;
                    }
                    if *lock(&cancellation.0) {
                        return Ok(());
                    }
                    self.inject(recorded.event)?;
                    update_held(&mut held, recorded.event);
                }
                let end = start
                    .checked_add(duration)
                    .ok_or("Recording duration is too large.")?;
                if !wait_until(cancellation, end) {
                    return Ok(());
                }
            }
            Ok(())
        })();
        let mut error = result.err();
        for input in held.into_iter().rev() {
            if let Err(release_error) = self.inject(input.release()) {
                let message = format!("Could not release synthetic input: {release_error}");
                error = Some(match error {
                    Some(previous) => format!("{previous}; {message}"),
                    None => message,
                });
            }
        }
        error
    }
}

fn is_control(key: Key) -> bool {
    matches!(key, Key::F8 | Key::F9 | Key::Escape)
}

fn position_for(event: EventType, mouse: Option<(f64, f64)>) -> Option<(f64, f64)> {
    match event {
        EventType::ButtonPress(_) | EventType::ButtonRelease(_) | EventType::Wheel { .. } => mouse,
        _ => None,
    }
}

fn update_held(held: &mut Vec<Held>, event: EventType) -> bool {
    let (input, pressed) = match event {
        EventType::KeyPress(key) => (Held::Key(key), true),
        EventType::KeyRelease(key) => (Held::Key(key), false),
        EventType::ButtonPress(button) => (Held::Button(button), true),
        EventType::ButtonRelease(button) => (Held::Button(button), false),
        _ => return true,
    };
    if pressed {
        if !held.contains(&input) {
            held.push(input);
        }
        true
    } else if let Some(index) = held.iter().position(|candidate| *candidate == input) {
        held.remove(index);
        true
    } else {
        false
    }
}

fn finish_recording(state: &mut State) {
    let offset = state
        .started
        .take()
        .map_or(Duration::ZERO, |start| start.elapsed());
    for input in state.held.drain(..).rev() {
        let event = input.release();
        state.events.push(RecordedEvent {
            offset,
            event,
            position: position_for(event, state.mouse),
        });
    }
    state.duration = offset;
    state.mode = Mode::Idle;
}

fn wait_until(cancellation: &Cancellation, deadline: Instant) -> bool {
    let mut cancelled = lock(&cancellation.0);
    while !*cancelled {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        let (guard, _) = cancellation
            .1
            .wait_timeout(cancelled, remaining)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cancelled = guard;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    type Output = Arc<Mutex<Vec<(Instant, EventType)>>>;

    fn recorder(delay: Duration) -> (Recorder, Output) {
        let output: Output = Arc::new(Mutex::new(Vec::new()));
        let sink = output.clone();
        (
            Recorder::with_simulator(
                Arc::new(move |event| {
                    lock(&sink).push((Instant::now(), *event));
                    Ok(())
                }),
                delay,
            ),
            output,
        )
    }

    fn seeded(recorder: &Recorder, events: &[(u64, EventType)], duration: u64) {
        let mut state = lock(&recorder.state);
        state.events = events
            .iter()
            .map(|(milliseconds, event)| RecordedEvent {
                offset: Duration::from_millis(*milliseconds),
                event: *event,
                position: None,
            })
            .collect();
        state.duration = Duration::from_millis(duration);
    }

    #[test]
    fn every_event_type_repeats_with_physical_keys_and_modifiers() {
        let (recorder, output) = recorder(Duration::ZERO);
        let events = [
            EventType::KeyPress(Key::ShiftRight),
            EventType::KeyPress(Key::Unknown(42)),
            EventType::MouseMove { x: 12.0, y: 34.0 },
            EventType::ButtonPress(Button::Unknown(4)),
            EventType::Wheel {
                delta_x: -2,
                delta_y: 3,
            },
            EventType::ButtonRelease(Button::Unknown(4)),
            EventType::KeyRelease(Key::Unknown(42)),
            EventType::KeyRelease(Key::ShiftRight),
        ];
        seeded(
            &recorder,
            &events.iter().map(|event| (0, *event)).collect::<Vec<_>>(),
            0,
        );
        recorder.play(3).unwrap();
        recorder.wait();
        let actual: Vec<_> = lock(&output).iter().map(|(_, event)| *event).collect();
        assert_eq!(actual, events.repeat(3));
        assert_eq!(recorder.snapshot().mode, Mode::Idle);
        assert!(recorder.snapshot().error.is_none());
    }

    #[test]
    fn capture_positions_and_balance_unmatched_releases() {
        let (recorder, _) = recorder(Duration::ZERO);
        recorder.handle_input(EventType::MouseMove { x: 23.0, y: 45.0 }, 1);
        recorder.toggle_record();
        recorder.handle_input(EventType::KeyRelease(Key::KeyA), 1);
        recorder.handle_input(EventType::ButtonRelease(Button::Right), 1);
        recorder.handle_input(EventType::KeyPress(Key::ControlLeft), 1);
        recorder.handle_input(EventType::KeyPress(Key::KeyA), 1);
        recorder.handle_input(EventType::ButtonPress(Button::Left), 1);
        recorder.handle_input(
            EventType::Wheel {
                delta_x: 1,
                delta_y: 2,
            },
            1,
        );
        recorder.stop();
        let state = lock(&recorder.state);
        assert_eq!(state.events.len(), 7);
        assert_eq!(state.events[2].position, Some((23.0, 45.0)));
        assert_eq!(state.events[3].position, Some((23.0, 45.0)));
        assert_eq!(
            state.events[4].event,
            EventType::ButtonRelease(Button::Left)
        );
        assert_eq!(state.events[4].position, Some((23.0, 45.0)));
        assert_eq!(state.events[5].event, EventType::KeyRelease(Key::KeyA));
        assert_eq!(
            state.events[6].event,
            EventType::KeyRelease(Key::ControlLeft)
        );
        assert!(
            state
                .events
                .windows(2)
                .all(|pair| pair[0].offset <= pair[1].offset)
        );
        assert_eq!(state.events[6].offset, state.duration);
        assert!(state.held.is_empty());
    }

    #[test]
    fn clicks_and_scroll_reposition_before_injection() {
        let (recorder, output) = recorder(Duration::ZERO);
        recorder.handle_input(EventType::MouseMove { x: 5.0, y: 6.0 }, 1);
        recorder.toggle_record();
        recorder.handle_input(EventType::ButtonPress(Button::Left), 1);
        recorder.handle_input(EventType::ButtonRelease(Button::Left), 1);
        recorder.handle_input(
            EventType::Wheel {
                delta_x: 0,
                delta_y: -1,
            },
            1,
        );
        recorder.stop();
        recorder.play(2).unwrap();
        recorder.wait();
        let events: Vec<_> = lock(&output).iter().map(|(_, event)| *event).collect();
        assert_eq!(events.len(), 12);
        for pair in events.as_chunks::<2>().0 {
            assert_eq!(pair[0], EventType::MouseMove { x: 5.0, y: 6.0 });
        }
    }

    #[test]
    fn invalid_empty_and_exclusive_modes() {
        let (recorder, output) = recorder(Duration::from_secs(1));
        assert!(recorder.play(1).is_err());
        assert!(recorder.play(0).is_err());
        recorder.toggle_record();
        assert!(recorder.play(1).is_err());
        assert_eq!(recorder.snapshot().mode, Mode::Recording);
        recorder.handle_input(EventType::KeyPress(Key::KeyA), 1);
        recorder.stop();
        assert!(recorder.play(0).is_err());
        recorder.play(1).unwrap();
        assert!(recorder.play(1).is_err());
        recorder.toggle_record();
        assert_eq!(recorder.snapshot().mode, Mode::Playing);
        recorder.handle_input(EventType::KeyPress(Key::KeyB), 1);
        assert_eq!(recorder.snapshot().count, 2);
        recorder.stop();
        recorder.wait();
        assert!(lock(&output).is_empty());
    }

    #[test]
    fn shortcut_exclusion_and_debounce() {
        let (recorder, output) = recorder(Duration::from_secs(1));
        recorder.handle_input(EventType::KeyPress(Key::F8), 1);
        recorder.handle_input(EventType::KeyPress(Key::F8), 1);
        assert_eq!(recorder.snapshot().mode, Mode::Recording);
        recorder.handle_input(EventType::KeyRelease(Key::F8), 1);
        recorder.handle_input(EventType::KeyPress(Key::KeyA), 1);
        recorder.handle_input(EventType::KeyPress(Key::Escape), 1);
        assert_eq!(recorder.snapshot().mode, Mode::Idle);
        recorder.handle_input(EventType::KeyRelease(Key::Escape), 1);
        recorder.handle_input(EventType::KeyPress(Key::F9), 1);
        recorder.handle_input(EventType::KeyPress(Key::F9), 1);
        assert_eq!(recorder.snapshot().mode, Mode::Playing);
        assert!(recorder.snapshot().error.is_none());
        recorder.handle_input(EventType::KeyPress(Key::Escape), 1);
        recorder.wait();
        recorder.handle_input(EventType::KeyRelease(Key::F9), 1);
        assert!(lock(&output).is_empty());
        assert!(
            lock(&recorder.state)
                .events
                .iter()
                .all(|recorded| match recorded.event {
                    EventType::KeyPress(key) | EventType::KeyRelease(key) => !is_control(key),
                    _ => true,
                })
        );
    }

    #[test]
    fn stop_interrupts_focus_delay_and_long_gaps() {
        let (recorder, output) = recorder(Duration::from_secs(1));
        seeded(&recorder, &[(0, EventType::KeyPress(Key::KeyA))], 0);
        let start = Instant::now();
        recorder.play(1).unwrap();
        recorder.stop();
        recorder.wait();
        assert!(start.elapsed() < Duration::from_millis(500));
        assert!(lock(&output).is_empty());

        let (recorder, output) = self::recorder(Duration::ZERO);
        seeded(
            &recorder,
            &[
                (0, EventType::KeyPress(Key::KeyA)),
                (10_000, EventType::KeyRelease(Key::KeyA)),
            ],
            10_000,
        );
        recorder.play(1).unwrap();
        let timeout = Instant::now() + Duration::from_secs(2);
        while lock(&output).is_empty() && Instant::now() < timeout {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!lock(&output).is_empty());
        let start = Instant::now();
        recorder.stop();
        recorder.wait();
        assert!(start.elapsed() < Duration::from_millis(500));
        let events: Vec<_> = lock(&output).iter().map(|(_, event)| *event).collect();
        assert_eq!(
            events,
            [
                EventType::KeyPress(Key::KeyA),
                EventType::KeyRelease(Key::KeyA)
            ]
        );
    }

    #[test]
    fn timestamps_and_final_pause_are_preserved_between_repeats() {
        let (recorder, output) = recorder(Duration::from_millis(20));
        seeded(
            &recorder,
            &[
                (15, EventType::KeyPress(Key::KeyA)),
                (40, EventType::KeyRelease(Key::KeyA)),
            ],
            70,
        );
        let start = Instant::now();
        recorder.play(2).unwrap();
        recorder.wait();
        let output = lock(&output);
        assert_eq!(output.len(), 4);
        assert!(output[0].0.duration_since(start) >= Duration::from_millis(35));
        assert!(output[1].0.duration_since(output[0].0) >= Duration::from_millis(20));
        assert!(output[2].0.duration_since(output[1].0) >= Duration::from_millis(40));
        assert!(start.elapsed() >= Duration::from_millis(160));
    }

    #[test]
    fn errors_cleanup_only_synthetic_holds_in_reverse_order() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let sink = output.clone();
        let recorder = Recorder::with_simulator(
            Arc::new(move |event| {
                lock(&sink).push(*event);
                if matches!(
                    event,
                    EventType::Wheel { .. } | EventType::KeyRelease(Key::KeyA)
                ) {
                    Err("injected failure".into())
                } else {
                    Ok(())
                }
            }),
            Duration::ZERO,
        );
        seeded(
            &recorder,
            &[
                (0, EventType::KeyPress(Key::ControlLeft)),
                (0, EventType::KeyPress(Key::KeyA)),
                (0, EventType::ButtonPress(Button::Left)),
                (
                    0,
                    EventType::Wheel {
                        delta_x: 0,
                        delta_y: 1,
                    },
                ),
                (0, EventType::KeyRelease(Key::KeyB)),
            ],
            0,
        );
        recorder.play(2).unwrap();
        recorder.wait();
        let output = lock(&output);
        assert_eq!(
            &output[4..],
            &[
                EventType::ButtonRelease(Button::Left),
                EventType::KeyRelease(Key::KeyA),
                EventType::KeyRelease(Key::ControlLeft),
            ]
        );
        assert_eq!(output.len(), 7);
        assert!(
            recorder
                .snapshot()
                .error
                .unwrap()
                .contains("Could not release")
        );
    }

    #[test]
    fn synchronous_callbacks_and_lingering_echoes_do_not_record() {
        let target: Arc<Mutex<Option<Recorder>>> = Arc::new(Mutex::new(None));
        let callback = target.clone();
        let recorder = Recorder::with_simulator(
            Arc::new(move |event| {
                let recorder = lock(&callback).clone().unwrap();
                recorder.handle_input(*event, 1);
                Ok(())
            }),
            Duration::ZERO,
        );
        *lock(&target) = Some(recorder.clone());
        seeded(
            &recorder,
            &[
                (0, EventType::KeyPress(Key::KeyA)),
                (0, EventType::KeyRelease(Key::KeyA)),
            ],
            0,
        );
        recorder.play(1).unwrap();
        recorder.wait();
        assert_eq!(recorder.snapshot().count, 2);
        *lock(&target) = None;

        let (recorder, _) = self::recorder(Duration::ZERO);
        seeded(
            &recorder,
            &[
                (0, EventType::KeyPress(Key::KeyA)),
                (0, EventType::KeyRelease(Key::KeyA)),
            ],
            0,
        );
        recorder.play(1).unwrap();
        recorder.wait();
        recorder.toggle_record();
        recorder.handle_input(EventType::KeyPress(Key::KeyA), 1);
        recorder.handle_input(EventType::KeyRelease(Key::KeyA), 1);
        assert_eq!(recorder.snapshot().count, 0);
        recorder.handle_input(EventType::KeyPress(Key::KeyB), 1);
        recorder.stop();
        assert_eq!(recorder.snapshot().count, 2);
    }

    #[test]
    fn capture_keeps_final_silence_and_balances_auto_repeat() {
        let (recorder, output) = recorder(Duration::ZERO);
        recorder.toggle_record();
        recorder.handle_input(EventType::KeyPress(Key::KeyA), 1);
        recorder.handle_input(EventType::KeyPress(Key::KeyA), 1);
        recorder.handle_input(EventType::KeyRelease(Key::KeyA), 1);
        recorder.handle_input(EventType::KeyRelease(Key::KeyA), 1);
        thread::sleep(Duration::from_millis(25));
        recorder.stop();
        {
            let state = lock(&recorder.state);
            assert_eq!(state.events.len(), 3);
            assert!(state.duration - state.events[2].offset >= Duration::from_millis(25));
        }
        recorder.play(2).unwrap();
        recorder.wait();
        assert_eq!(lock(&output).len(), 6);
    }

    #[test]
    fn reentrant_escape_cancels_and_releases_in_reverse_order() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let sink = output.clone();
        let target: Arc<Mutex<Option<Recorder>>> = Arc::new(Mutex::new(None));
        let callback = target.clone();
        let recorder = Recorder::with_simulator(
            Arc::new(move |event| {
                lock(&sink).push(*event);
                let recorder = lock(&callback).clone().unwrap();
                recorder.handle_input(*event, 1);
                if *event == EventType::ButtonPress(Button::Right) {
                    recorder.handle_input(EventType::KeyPress(Key::Escape), 1);
                }
                Ok(())
            }),
            Duration::ZERO,
        );
        *lock(&target) = Some(recorder.clone());
        seeded(
            &recorder,
            &[
                (0, EventType::KeyPress(Key::ShiftLeft)),
                (0, EventType::KeyPress(Key::KeyA)),
                (0, EventType::ButtonPress(Button::Right)),
                (0, EventType::KeyPress(Key::KeyB)),
            ],
            0,
        );
        recorder.play(2).unwrap();
        recorder.wait();
        assert_eq!(
            &lock(&output)[3..],
            &[
                EventType::ButtonRelease(Button::Right),
                EventType::KeyRelease(Key::KeyA),
                EventType::KeyRelease(Key::ShiftLeft),
            ]
        );
        assert!(recorder.snapshot().error.is_none());
        *lock(&target) = None;
    }

    #[test]
    fn failed_press_does_not_release_physical_or_uninjected_keys() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let sink = output.clone();
        let recorder = Recorder::with_simulator(
            Arc::new(move |event| {
                lock(&sink).push(*event);
                if *event == EventType::KeyPress(Key::KeyB) {
                    Err("press failed".into())
                } else {
                    Ok(())
                }
            }),
            Duration::ZERO,
        );
        seeded(
            &recorder,
            &[
                (0, EventType::KeyPress(Key::KeyA)),
                (0, EventType::KeyPress(Key::KeyB)),
            ],
            0,
        );
        recorder.play(1).unwrap();
        recorder.wait();
        assert_eq!(
            *lock(&output),
            [
                EventType::KeyPress(Key::KeyA),
                EventType::KeyPress(Key::KeyB),
                EventType::KeyRelease(Key::KeyA),
            ]
        );
        assert_eq!(recorder.snapshot().error.as_deref(), Some("press failed"));
    }

    #[test]
    fn expired_echo_receipts_do_not_swallow_physical_input() {
        let (recorder, _) = recorder(Duration::ZERO);
        lock(&recorder.state).echoes.push_back((
            Instant::now() - Duration::from_millis(1),
            EventType::KeyPress(Key::KeyA),
        ));
        recorder.toggle_record();
        recorder.handle_input(EventType::KeyPress(Key::KeyA), 1);
        recorder.stop();
        assert_eq!(recorder.snapshot().count, 2);
    }
}
