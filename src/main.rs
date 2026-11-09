mod recorder;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};
use std::thread;
use std::time::Duration;

use eframe::egui;
use recorder::{Mode, Recorder};

struct SimonSays {
    recorder: Recorder,
    repeats: Arc<AtomicU32>,
    repeat_text: String,
    notice: Option<String>,
    listener_error: Arc<Mutex<Option<String>>>,
    closing: Arc<Mutex<bool>>,
    previous_mode: Mode,
}

impl SimonSays {
    fn new(ctx: &egui::Context) -> Self {
        let recorder = Recorder::new();
        let repeats = Arc::new(AtomicU32::new(1));
        let listener_error = Arc::new(Mutex::new(None));
        let closing = Arc::new(Mutex::new(false));
        let input_recorder = recorder.clone();
        let input_repeats = Arc::clone(&repeats);
        let input_error = Arc::clone(&listener_error);
        let input_closing = Arc::clone(&closing);
        let repaint = ctx.clone();
        let failure_recorder = recorder.clone();
        thread::spawn(move || {
            let input_repaint = repaint.clone();
            let result = rdev::listen(move |event| {
                let closing = input_closing.lock().unwrap();
                if !*closing {
                    input_recorder
                        .handle_input(event.event_type, input_repeats.load(Ordering::Acquire));
                    input_repaint.request_repaint();
                }
            });
            failure_recorder.stop();
            set_listener_error(&input_error, result);
            repaint.request_repaint();
        });
        Self {
            recorder,
            repeats,
            repeat_text: "1".to_owned(),
            notice: None,
            listener_error,
            closing,
            previous_mode: Mode::Idle,
        }
    }

    fn sync_window(&mut self, ctx: &egui::Context) {
        let mode = self.recorder.snapshot().mode;
        if self.listener_error.lock().unwrap().is_some() {
            self.recorder.stop();
        }
        if mode != self.previous_mode {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(mode != Mode::Idle));
            self.previous_mode = mode;
            self.notice = None;
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

fn set_listener_error(error: &Mutex<Option<String>>, result: Result<(), rdev::ListenError>) {
    *error.lock().unwrap() = Some(match result {
        Ok(()) => "Input listener stopped. Restart SimonSays.".to_owned(),
        Err(error) => format!("Input listener failed: {error:?}. Check desktop permissions."),
    });
}

fn parse_repeats(text: &str) -> Result<u32, &'static str> {
    text.trim()
        .parse::<u32>()
        .ok()
        .filter(|count| *count > 0)
        .ok_or("Repeat count must be a positive whole number.")
}

impl eframe::App for SimonSays {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_window(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("SimonSays");
            ui.label("F8: record / stop    F9: play    Esc: stop");
            ui.add_space(12.0);

            let snapshot = self.recorder.snapshot();
            let listener_error = self.listener_error.lock().unwrap().clone();
            let ready = listener_error.is_none();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        ready && snapshot.mode != Mode::Playing,
                        egui::Button::new("Record (F8)"),
                    )
                    .clicked()
                {
                    self.recorder.toggle_record();
                }
                if ui
                    .add_enabled(snapshot.mode != Mode::Idle, egui::Button::new("Stop (Esc)"))
                    .clicked()
                {
                    self.recorder.stop();
                }
                if ui
                    .add_enabled(
                        ready && snapshot.mode == Mode::Idle,
                        egui::Button::new("Play (F9)"),
                    )
                    .clicked()
                {
                    self.notice = parse_repeats(&self.repeat_text)
                        .map_err(str::to_owned)
                        .and_then(|count| self.recorder.play(count))
                        .err();
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Repeat count:");
                let response = ui.add_enabled(
                    snapshot.mode == Mode::Idle,
                    egui::TextEdit::singleline(&mut self.repeat_text).desired_width(70.0),
                );
                if response.changed() {
                    match parse_repeats(&self.repeat_text) {
                        Ok(count) => {
                            self.repeats.store(count, Ordering::Release);
                            self.notice = None;
                        }
                        Err(message) => {
                            // Invalid text also disables playback through F9.
                            self.repeats.store(0, Ordering::Release);
                            self.notice = Some(message.to_owned());
                        }
                    }
                }
            });
            ui.add_space(12.0);
            let message = listener_error
                .or(snapshot.error)
                .or_else(|| self.notice.clone())
                .unwrap_or_else(|| match snapshot.mode {
                    Mode::Idle => format!("Ready — {} steps recorded.", snapshot.count),
                    Mode::Recording => {
                        format!("Recording — {} steps. F8 or Esc to stop.", snapshot.count)
                    }
                    Mode::Playing => "Playing — starts after 1 second. Esc to stop.".to_owned(),
                });
            ui.label(message);
        });
        self.sync_window(ui.ctx());
    }
}

impl Drop for SimonSays {
    fn drop(&mut self) {
        {
            let mut closing = self.closing.lock().unwrap();
            *closing = true;
            self.recorder.stop();
        }
        self.recorder.wait();
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([420.0, 200.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "SimonSays",
        options,
        Box::new(|context| Ok(Box::new(SimonSays::new(&context.egui_ctx)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_count_must_be_positive_and_fit_u32() {
        assert_eq!(parse_repeats(" 2 "), Ok(2));
        assert_eq!(parse_repeats("1"), Ok(1));
        for invalid in ["", "0", "-1", "1.5", "abc", "4294967296"] {
            assert!(parse_repeats(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn listener_exit_is_visible_even_if_it_returns_without_error() {
        let error = Mutex::new(None);
        set_listener_error(&error, Ok(()));
        assert!(error.lock().unwrap().as_ref().unwrap().contains("stopped"));
    }
}
