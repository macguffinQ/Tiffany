// Adapted from OpenAI Codex CLI:
// https://github.com/openai/codex/blob/main/codex-rs/tui/src/tui/event_stream.rs
//
// OpenAI Codex CLI is licensed under Apache-2.0. This local version keeps the
// same event-stream direction for tiffany-loop lightweight, non-fullscreen runner:
// terminal input and draw notifications enter one async event path, and the
// crossterm stream can be dropped while handing stdin to external programs.

use crossterm::event::Event;
use crossterm::event::EventStream;
use crossterm::event::KeyEvent;
use futures::StreamExt;
use std::io;
use tokio::sync::broadcast;

#[derive(Debug)]
pub(super) enum TuiEvent {
    Key(KeyEvent),
    Paste(String),
    Resize,
    Draw,
    Closed,
}

pub(super) struct TuiEventStream {
    events: Option<EventStream>,
    draw_rx: broadcast::Receiver<()>,
    poll_draw_first: bool,
}

impl TuiEventStream {
    pub(super) fn new(draw_rx: broadcast::Receiver<()>) -> Self {
        Self {
            events: Some(EventStream::new()),
            draw_rx,
            poll_draw_first: false,
        }
    }

    pub(super) fn pause_events(&mut self) {
        self.events = None;
    }

    pub(super) fn resume_events(&mut self) {
        if self.events.is_none() {
            self.events = Some(EventStream::new());
        }
    }

    pub(super) async fn next(&mut self) -> io::Result<TuiEvent> {
        loop {
            if self.poll_draw_first {
                self.poll_draw_first = false;
                if let Some(event) = self.try_recv_draw() {
                    return Ok(event);
                }
            } else {
                self.poll_draw_first = true;
            }

            let events = &mut self.events;
            let draw_rx = &mut self.draw_rx;
            tokio::select! {
                draw = draw_rx.recv() => {
                    match draw {
                        Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => return Ok(TuiEvent::Draw),
                        Err(broadcast::error::RecvError::Closed) => return Ok(TuiEvent::Closed),
                    }
                }
                event = next_crossterm_event(events) => {
                    match event? {
                        Some(event) => return Ok(event),
                        None => continue,
                    }
                }
            }
        }
    }

    fn try_recv_draw(&mut self) -> Option<TuiEvent> {
        match self.draw_rx.try_recv() {
            Ok(()) | Err(broadcast::error::TryRecvError::Lagged(_)) => Some(TuiEvent::Draw),
            Err(broadcast::error::TryRecvError::Empty) => None,
            Err(broadcast::error::TryRecvError::Closed) => Some(TuiEvent::Closed),
        }
    }
}

async fn next_crossterm_event(events: &mut Option<EventStream>) -> io::Result<Option<TuiEvent>> {
    let Some(events) = events.as_mut() else {
        std::future::pending::<()>().await;
        return Ok(None);
    };

    match events.next().await {
        Some(Ok(event)) => Ok(map_crossterm_event(event)),
        Some(Err(err)) => Err(err),
        None => Ok(None),
    }
}

fn map_crossterm_event(event: Event) -> Option<TuiEvent> {
    match event {
        Event::Key(key_event) => Some(TuiEvent::Key(key_event)),
        Event::Resize(_, _) => Some(TuiEvent::Resize),
        Event::Paste(pasted) => Some(TuiEvent::Paste(pasted)),
        Event::FocusGained => Some(TuiEvent::Draw),
        Event::FocusLost => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyModifiers;

    #[test]
    fn maps_key_paste_resize_and_focus_gain() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        assert!(matches!(
            map_crossterm_event(Event::Key(key)),
            Some(TuiEvent::Key(_))
        ));
        assert!(matches!(
            map_crossterm_event(Event::Paste("hello".into())),
            Some(TuiEvent::Paste(text)) if text == "hello"
        ));
        assert!(matches!(
            map_crossterm_event(Event::Resize(120, 40)),
            Some(TuiEvent::Resize)
        ));
        assert!(matches!(
            map_crossterm_event(Event::FocusGained),
            Some(TuiEvent::Draw)
        ));
        assert!(map_crossterm_event(Event::FocusLost).is_none());
    }
}
