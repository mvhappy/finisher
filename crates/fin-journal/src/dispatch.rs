//! The writer's control side (plan D, Decision 18): control messages in order with the
//! data. Each carries the count of data messages queued before it; the writer handles exactly
//! those first, and reads the next control message ahead so none of the data queued after it
//! is handled before it.

use crate::JournalError;
use crate::control::{Control, Data};
use crate::writer::Writer;

impl Writer {
    /// Handles every data message queued (close).
    pub fn drain_data(&mut self) {
        while let Ok(d) = self.data.try_recv() {
            self.on_data(d);
        }
    }

    /// Handles the data queued before a control message (`seen` messages in all), so it
    /// sees every entry recorded before it and none recorded after.
    pub fn catch_up(&mut self, seen: u64) {
        while self.done < seen {
            match self.data.try_recv() {
                Ok(d) => self.on_data(d),
                Err(_) => break,
            }
        }
    }

    /// Whether the next control message came before the data not handled yet (it saw at
    /// most `done` data messages): it is handled first.
    pub fn control_due(&mut self) -> bool {
        if self.next_ctl.is_none() {
            self.next_ctl = self.ctl.try_recv().ok();
        }
        self.next_ctl
            .as_ref()
            .is_some_and(|(seen, _)| *seen <= self.done)
    }

    /// One data message in channel order with the control messages: those sent before it
    /// are handled first (a close after it). False once the journal is closed.
    pub fn take_data(&mut self, d: Data) -> bool {
        while self.control_due() {
            if matches!(self.next_ctl, Some((_, Control::Close(_)))) {
                self.on_data(d);
                return self.drain_control();
            }
            let (seen, c) = self.next_ctl.take().expect("control_due holds one");
            if !self.handle_control(seen, c) {
                return false;
            }
        }
        self.on_data(d);
        true
    }

    pub fn drain_control(&mut self) -> bool {
        while let Some((seen, c)) = self.next_ctl.take().or_else(|| self.ctl.try_recv().ok()) {
            if !self.handle_control(seen, c) {
                return false;
            }
        }
        true
    }

    /// A control message, after the data queued before it. False on close.
    pub fn handle_control(&mut self, seen: u64, c: Control) -> bool {
        {
            self.catch_up(seen);
            match c {
                Control::Sync { entry, slot, ack } => {
                    if slot.take() {
                        let ok = self.write(*entry) && self.flush().is_ok();
                        let _ = ack.try_send(ok);
                    }
                }
                Control::Flush(ack) => {
                    self.maintain();
                    let _ = ack.try_send(self.flush());
                }
                Control::Closed(ack) => {
                    self.maintain();
                    let _ = self.flush();
                    let open = usize::from(self.active.is_some() && !self.files.is_empty());
                    let n = self.files.len() - open;
                    let _ = ack.try_send(self.files[..n].iter().map(|f| f.name.clone()).collect());
                }
                Control::Consent { from, to } => self.on_consent(from, to),
                Control::Caps => {
                    let now = self.now();
                    if (self.scanned || self.listed)
                        && self.suspended.is_none()
                        && let Err(e) = self.apply_caps(now)
                    {
                        self.suspend(&e);
                    }
                }
                Control::Project { id, policy } => self.on_project(id, policy),
                Control::Purge { sel, ack } => {
                    let r = self.purge(&sel).map_err(|e| {
                        let code = crate::disk::code_of(&e);
                        self.suspend(&e);
                        JournalError::new("JOURNAL_UNAVAILABLE", format!("{code}: {e}"))
                    });
                    let _ = ack.try_send(r);
                }
                Control::Close(ack) => {
                    self.shutdown();
                    let _ = ack.try_send(());
                    return false;
                }
            }
        }
        true
    }
}
