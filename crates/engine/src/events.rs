//! What happened, for an agent watching a user (control `events.subscribe` / `events.poll`, MCP
//! notifications): commands started, ended and run as they happen; the drawing changed (with
//! the objects that changed), the selection, the prompt, and drawings opened, closed or switched
//! found when the events are read, so a burst of changes comes as one event of each kind; and
//! dialogs opened (told by the app). Nothing is recorded until something subscribes.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use cadaddy_doc::{Entity, Handle};
use serde_json::{Value, json};

use crate::Session;

/// Events kept for a reader (the oldest go first, counted in `dropped`).
const MAX_EVENTS: usize = 1_000;
/// Handles listed per event.
const MAX_HANDLES: usize = 200;

/// The events and what the last read saw.
#[derive(Clone, Debug, Default)]
pub struct EventLog {
    pub on: bool,
    seq: u64,
    queue: VecDeque<(u64, Value)>,
    dropped: u64,
    seen: Seen,
}

/// The state the last read compared against.
#[derive(Clone, Debug, Default)]
struct Seen {
    docs: Vec<u64>,
    active: Option<u64>,
    selection: Vec<Handle>,
    prompt: String,
    /// Per drawing: its revision and model objects then.
    drawings: HashMap<u64, (u64, HashMap<Handle, Arc<Entity>>)>,
}

fn objects(s: &crate::DocState) -> HashMap<Handle, Arc<Entity>> {
    s.doc.model.iter().map(|e| (e.handle, Arc::clone(e))).collect()
}

fn listed(v: &[Handle]) -> Vec<String> {
    v.iter().take(MAX_HANDLES).map(|h| h.hex()).collect()
}

impl Session {
    /// Record event `kind` with `data` (when someone is subscribed).
    pub fn event(&mut self, kind: &str, data: Value) {
        if !self.events.on {
            return;
        }
        let log = &mut self.events;
        log.seq += 1;
        let mut ev = json!({ "seq": log.seq, "type": kind });
        if let (Some(o), Value::Object(d)) = (ev.as_object_mut(), data) {
            o.extend(d);
        }
        log.queue.push_back((log.seq, ev));
        while log.queue.len() > MAX_EVENTS {
            log.queue.pop_front();
            log.dropped += 1;
        }
    }

    /// Start (or stop) recording events. Starting takes the current state as seen.
    pub fn events_subscribe(&mut self, on: bool) -> Value {
        if on && !self.events.on {
            self.events.on = true;
            self.events.seen = Seen::default();
            self.look();
            // What is open now is the baseline, not news.
            self.events.queue.clear();
        }
        if !on {
            self.events = EventLog::default();
        }
        json!({ "subscribed": self.events.on, "seq": self.events.seq })
    }

    /// The events after `since` (changes found by comparing with the last read first).
    pub fn events_poll(&mut self, since: u64) -> Value {
        if !self.events.on {
            return json!({ "subscribed": false, "events": [], "next": since });
        }
        self.look();
        let events: Vec<Value> = self.events.queue.iter().filter(|(s, _)| *s > since).map(|(_, e)| e.clone()).collect();
        json!({ "subscribed": true, "events": events, "next": self.events.seq, "dropped": self.events.dropped })
    }

    /// Compare the session with what the last read saw and record what changed.
    fn look(&mut self) {
        let docs: Vec<u64> = self.docs.iter().map(|d| d.uid).collect();
        let active = self.state().ok().map(|s| s.uid);
        let titles: HashMap<u64, String> = self.docs.iter().map(|d| (d.uid, d.title.clone())).collect();
        let seen = std::mem::take(&mut self.events.seen);
        let first = seen.docs.is_empty() && seen.active.is_none() && seen.drawings.is_empty();
        if !first {
            for uid in docs.iter().filter(|u| !seen.docs.contains(u)) {
                self.event("document.opened", json!({ "title": titles.get(uid) }));
            }
            for uid in seen.docs.iter().filter(|u| !docs.contains(u)) {
                self.event("document.closed", json!({ "uid": uid }));
            }
            if active != seen.active && active.is_some() && seen.docs.contains(&active.unwrap_or_default()) {
                self.event("document.switched", json!({ "title": active.and_then(|a| titles.get(&a)) }));
            }
        }
        // The active drawing's changes (coalesced since the last read).
        let mut drawings = seen.drawings;
        drawings.retain(|uid, _| docs.contains(uid));
        if let Ok(st) = self.state() {
            let rev = st.revision;
            let uid = st.uid;
            match drawings.get(&uid) {
                Some((r, before)) if *r != rev => {
                    let now = objects(st);
                    let mut added: Vec<Handle> = now.keys().filter(|h| !before.contains_key(h)).copied().collect();
                    let mut removed: Vec<Handle> = before.keys().filter(|h| !now.contains_key(h)).copied().collect();
                    let mut changed: Vec<Handle> =
                        now.iter().filter(|(h, e)| before.get(h).is_some_and(|b| !Arc::ptr_eq(b, e))).map(|(h, _)| *h).collect();
                    for v in [&mut added, &mut removed, &mut changed] {
                        v.sort_unstable_by_key(|h| h.0);
                    }
                    drawings.insert(uid, (rev, now));
                    if !(added.is_empty() && removed.is_empty() && changed.is_empty()) {
                        let counts = [added.len(), removed.len(), changed.len()];
                        self.event(
                            "drawing.changed",
                            json!({ "added": listed(&added), "removed": listed(&removed), "changed": listed(&changed), "counts": counts }),
                        );
                    }
                }
                Some(_) => {}
                None => {
                    drawings.insert(uid, (rev, objects(st)));
                }
            }
        }
        let selection = self.selection();
        if !first && selection != seen.selection {
            self.event("selection.changed", json!({ "count": selection.len(), "handles": listed(&selection) }));
        }
        let prompt = self.prompt_text();
        if !first && prompt != seen.prompt {
            self.event("prompt.changed", json!({ "prompt": prompt }));
        }
        self.events.seen = Seen { docs, active, selection, prompt, drawings };
    }
}
