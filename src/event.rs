use std::{path::PathBuf, thread, time::Duration};

use anyhow::Result;
use crossbeam_channel::{unbounded, Receiver, Sender};
use crossterm::event::{self, Event as CtEvent};

pub enum AppEvent {
    Input(CtEvent),
    FsChanged(Vec<PathBuf>),
    Tick,
}

pub struct EventLoop {
    pub rx: Receiver<AppEvent>,
    pub tx: Sender<AppEvent>,
}

impl EventLoop {
    pub fn new() -> Result<Self> {
        let (tx, rx) = unbounded();

        // Input thread.
        {
            let tx = tx.clone();
            thread::spawn(move || loop {
                if let Ok(true) = event::poll(Duration::from_millis(200)) {
                    if let Ok(ev) = event::read() {
                        if tx.send(AppEvent::Input(ev)).is_err() {
                            break;
                        }
                    }
                } else {
                    // idle tick so app can do periodic work if needed
                    if tx.send(AppEvent::Tick).is_err() {
                        break;
                    }
                }
            });
        }

        Ok(Self { rx, tx })
    }
}
