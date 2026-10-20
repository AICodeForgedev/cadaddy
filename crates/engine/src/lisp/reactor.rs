//! Reactors, the basics of Visual LISP's: a command reactor (`vlr-command-reactor`) calls its
//! functions when a command starts, ends, is cancelled or fails; an editor reactor
//! (`vlr-editor-reactor`) on those events and on saves; an object reactor (`vlr-object-reactor`)
//! when one of its objects is modified or erased.
//!
//! A reaction is `(event . function)`; the function gets the reactor and the event's arguments:
//! a command reactor `(reactor ("LINE"))`, an object reactor `(object reactor nil)`. Callbacks run
//! to the end, one at a time, between commands; they don't fire while a routine runs (its own
//! `command` calls), and `command` inside a callback is refused, so a callback can't set off
//! another.

use std::collections::BTreeMap;
use std::sync::Arc;

use cadaddy_doc::{Entity, Handle};

use super::LispError;
use super::builtins::{arity, list_arg};
use super::eval::{Ctx, Flow, Machine};
use super::value::{ReactorKind, Value};
use crate::Session;

type F = Result<Flow, LispError>;

/// Reactors in one drawing, at most.
const MAX_REACTORS: usize = 10_000;
/// Objects one object reactor watches, at most.
const MAX_OWNERS: usize = 100_000;
/// Callbacks one event runs, at most.
const MAX_CALLS: usize = 1_000;

/// A reactor: what it reacts to and what it calls.
#[derive(Clone, Debug)]
pub struct Reactor {
    kind: ReactorKind,
    data: Value,
    /// `(event . function)`: the event's name in upper case.
    reactions: Vec<(String, Value)>,
    /// An object reactor's objects, each with the entity as last seen (`None`: erased).
    owners: Vec<(Handle, Option<Arc<Entity>>)>,
    /// Removed (`vlr-remove`) reactors keep their settings and come back with `vlr-add`.
    added: bool,
}

/// Command events, as reactions name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandEvent {
    WillStart,
    Ended,
    Cancelled,
    Failed,
}

impl CommandEvent {
    fn name(self) -> &'static str {
        match self {
            CommandEvent::WillStart => ":VLR-COMMANDWILLSTART",
            CommandEvent::Ended => ":VLR-COMMANDENDED",
            CommandEvent::Cancelled => ":VLR-COMMANDCANCELLED",
            CommandEvent::Failed => ":VLR-COMMANDFAILED",
        }
    }
}

fn reactor_arg(v: &Value) -> Result<u32, LispError> {
    match v {
        Value::Reactor(id, _) => Ok(*id),
        _ => Err(LispError::bad_arg("VLR-object", v)),
    }
}

/// `((event . function) …)` as given to a constructor.
fn reactions(v: &Value) -> Result<Vec<(String, Value)>, LispError> {
    list_arg(v)?
        .into_iter()
        .map(|pair| match (pair.car(), pair.cdr()) {
            (Value::Sym(e), f) if !f.is_nil() => Ok((e.name().to_ascii_uppercase(), f)),
            _ => Err(LispError::bad_arg("reaction (event . function)", &pair)),
        })
        .collect()
}

fn make(ctx: &mut Ctx<'_>, kind: ReactorKind, data: Value, reactions: Vec<(String, Value)>, owners: Vec<Handle>) -> F {
    if ctx.env.reactors.len() >= MAX_REACTORS {
        return Err(LispError::new(format!("more than {MAX_REACTORS} reactors")));
    }
    let doc = ctx.s.doc().ok();
    let owners = owners.into_iter().map(|h| (h, doc.and_then(|d| d.entity(h)).cloned())).collect();
    ctx.env.next_reactor = ctx.env.next_reactor.checked_add(1).ok_or_else(|| LispError::new("too many reactors"))?;
    let id = ctx.env.next_reactor;
    ctx.env.reactors.insert(id, Reactor { kind, data, reactions, owners, added: true });
    Ok(Flow::Value(Value::Reactor(id, kind)))
}

/// `(vlr-command-reactor data reactions)`.
pub fn command_reactor(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let r = reactions(&a[1])?;
    make(ctx, ReactorKind::Command, a[0].clone(), r, Vec::new())
}

/// `(vlr-editor-reactor data reactions)`.
pub fn editor_reactor(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let r = reactions(&a[1])?;
    make(ctx, ReactorKind::Editor, a[0].clone(), r, Vec::new())
}

/// `(vlr-object-reactor owners data reactions)`: owners are entity names.
pub fn object_reactor(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 3, 3)?;
    let owners = list_arg(&a[0])?;
    if owners.len() > MAX_OWNERS {
        return Err(LispError::new(format!("an object reactor watches at most {MAX_OWNERS} objects")));
    }
    let owners = owners
        .iter()
        .map(|o| match o {
            Value::Ename(h) => Ok(*h),
            other => Err(LispError::bad_arg("ename", other)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let r = reactions(&a[2])?;
    make(ctx, ReactorKind::Object, a[1].clone(), r, owners)
}

fn with<T>(ctx: &mut Ctx<'_>, v: &Value, f: impl FnOnce(&mut Reactor) -> T) -> Result<T, LispError> {
    let id = reactor_arg(v)?;
    ctx.env.reactors.get_mut(&id).map(f).ok_or_else(|| LispError::bad_arg("VLR-object", v))
}

/// `(vlr-remove reactor)`: it stops reacting (`vlr-add` brings it back).
pub fn remove(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let was = with(ctx, &a[0], |r| std::mem::replace(&mut r.added, false))?;
    Ok(Flow::Value(if was { a[0].clone() } else { Value::Nil }))
}

/// `(vlr-add reactor)`.
pub fn add(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let was = with(ctx, &a[0], |r| std::mem::replace(&mut r.added, true))?;
    Ok(Flow::Value(if was { Value::Nil } else { a[0].clone() }))
}

/// `(vlr-added-p reactor)`.
pub fn added_p(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    Ok(Flow::Value(Value::bool(with(ctx, &a[0], |r| r.added)?)))
}

/// `(vlr-data reactor)`.
pub fn data(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    Ok(Flow::Value(with(ctx, &a[0], |r| r.data.clone())?))
}

/// `(vlr-data-set reactor data)`.
pub fn data_set(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let d = a[1].clone();
    with(ctx, &a[0], |r| r.data = d.clone())?;
    Ok(Flow::Value(d))
}

/// `(vlr-type reactor)`: `:VLR-Command-Reactor` …
pub fn type_of(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let k = with(ctx, &a[0], |r| r.kind)?;
    Ok(Flow::Value(Value::sym(&format!(":{}", k.type_name()))))
}

/// `(vlr-reactions reactor)`: `((event . function) …)`.
pub fn reactions_of(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let r = with(ctx, &a[0], |r| r.reactions.clone())?;
    Ok(Flow::Value(Value::list(r.into_iter().map(|(e, f)| Value::cons(Value::sym(&e), f)).collect::<Vec<_>>())))
}

/// `(vlr-owners reactor)`.
pub fn owners(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let o = with(ctx, &a[0], |r| r.owners.iter().map(|(h, _)| Value::Ename(*h)).collect::<Vec<_>>())?;
    Ok(Flow::Value(Value::list(o)))
}

/// `(vlr-reactors [type …])`: `((:VLR-Command-Reactor r …) …)` for reactors that are added.
pub fn reactors(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    let wanted: Vec<String> = a.iter().filter_map(|v| if let Value::Sym(s) = v { Some(s.name().to_ascii_uppercase()) } else { None }).collect();
    let mut out = Vec::new();
    for kind in [ReactorKind::Command, ReactorKind::Editor, ReactorKind::Object] {
        let name = format!(":{}", kind.type_name());
        if !wanted.is_empty() && !wanted.contains(&name) {
            continue;
        }
        let these: Vec<Value> = ctx.env.reactors.iter().filter(|(_, r)| r.kind == kind && r.added).map(|(id, _)| Value::Reactor(*id, kind)).collect();
        if !these.is_empty() {
            out.push(Value::cons(Value::sym(&name), Value::list(these)));
        }
    }
    Ok(Flow::Value(Value::list(out)))
}

/// `(vlr-remove-all [type])`: remove every reactor (of a type).
pub fn remove_all(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 0, 1)?;
    let wanted = match a.first() {
        Some(Value::Sym(s)) => Some(s.name().to_ascii_uppercase()),
        _ => None,
    };
    for r in ctx.env.reactors.values_mut() {
        if wanted.as_deref().is_none_or(|w| w == format!(":{}", r.kind.type_name())) {
            r.added = false;
        }
    }
    Ok(Flow::Value(Value::Nil))
}

/// The calls an event makes.
type Calls = Vec<(Value, Vec<Value>)>;

fn callable(env: &super::Env, f: &Value) -> Value {
    match f {
        Value::Sym(s) => super::eval::lookup(env, s),
        other => other.clone(),
    }
}

impl Session {
    /// A command started, ended, was cancelled or failed: the command and editor reactors of the
    /// active drawing react; after the end of a command, object reactors whose objects it
    /// changed do too.
    pub(crate) fn lisp_command_event(&mut self, event: CommandEvent, command: &str) {
        if self.lisp.active || self.lisp.in_reactor {
            return;
        }
        let Ok((uid, doc)) = self.state().map(|s| (s.uid, s.doc.clone())) else { return };
        let Some(env) = self.lisp.envs.get_mut(&uid) else { return };
        if env.reactors.is_empty() {
            return;
        }
        let name = event.name();
        let mut calls: Calls = Vec::new();
        for (id, r) in &env.reactors {
            if !r.added || r.kind == ReactorKind::Object {
                continue;
            }
            for (e, f) in &r.reactions {
                if e == name {
                    calls.push((f.clone(), vec![Value::Reactor(*id, r.kind), Value::list(vec![Value::str(&command.to_ascii_uppercase())])]));
                }
            }
        }
        if event != CommandEvent::WillStart {
            for (id, r) in env.reactors.iter_mut() {
                if r.kind != ReactorKind::Object {
                    continue;
                }
                let mut changed = Vec::new();
                for (h, seen) in &mut r.owners {
                    let now = doc.entity(*h);
                    let same = match (now, seen.as_ref()) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
                        (None, None) => true,
                        _ => false,
                    };
                    if !same {
                        changed.push((*h, if now.is_some() { ":VLR-MODIFIED" } else { ":VLR-ERASED" }));
                        *seen = now.cloned();
                    }
                }
                if !r.added {
                    continue;
                }
                for (h, ev) in changed {
                    for (e, f) in &r.reactions {
                        if e == ev {
                            calls.push((f.clone(), vec![Value::Ename(h), Value::Reactor(*id, ReactorKind::Object), Value::Nil]));
                        }
                    }
                }
            }
        }
        let calls: Calls = calls.into_iter().take(MAX_CALLS).map(|(f, args)| (callable(env, &f), args)).collect();
        self.lisp_react(calls);
    }

    /// Run reactors' callbacks to the end, one after the other.
    fn lisp_react(&mut self, calls: Calls) {
        if calls.is_empty() {
            return;
        }
        self.lisp.in_reactor = true;
        for (f, args) in calls {
            if !matches!(f, Value::Func(_) | Value::Subr(_)) {
                continue;
            }
            let _ = self.lisp_start(Machine::calling(f, args), false, true, Vec::new(), "LISP".into());
        }
        self.lisp.in_reactor = false;
    }
}

/// The drawing's reactors, by id.
pub type Reactors = BTreeMap<u32, Reactor>;
