//! FILTER (object selection filters): selects the objects whose properties pass a list of
//! conditions joined with AND / OR / XOR / NOT, as AutoCAD's filter lists do with their
//! "**Begin AND" … "**End AND" groups. Typed, the filter is one line:
//! `type = Line and (layer = Walls or color = red) and not lineweight > 0.3`.
//! In JSON it is that line or a tree: `{"and": [{"property": "type", "op": "=", "value": "Line"},
//! {"not": {…}}]}`.

use std::collections::HashMap;

use cadaddy_doc::{Drawing, Entity, Handle};
use serde_json::{Value, json};

use super::super::qselect::{self, Op};
use super::super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// Most conditions and groups one filter holds, and how deep groups nest.
const MAX_NODES: usize = 256;
const MAX_DEPTH: usize = 32;
/// Most objects one FILTER looks at.
const MAX_SCAN: usize = 2_000_000;
/// Longest typed filter.
const MAX_TEXT: usize = 8192;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("filter", "Object Selection Filters", run_filter)
            .alias(&["fi"])
            .params(
                "{filter: \"type = Line and (layer = Walls or color = red)\" | {and|or|xor: [filters]} | {not: filter} | \
                 {property, op?: =|!=|<|>|<=|>=|* (wildcard; default =), value}, applyTo?: drawing|selection, mode?: new|append|exclude} \
                 → {selected, handles} (properties as QSELECT names them: type, layer, color, linetype, lineweight, radius, …)",
            )
            .noundo()
            .interactive(|_| Ok(Box::new(FilterM))),
    ]
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cond {
    Test { property: String, op: CmpOp, value: Value },
    And(Vec<Cond>),
    Or(Vec<Cond>),
    Xor(Vec<Cond>),
    Not(Box<Cond>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CmpOp {
    Base(Op),
    Le,
    Ge,
}

impl CmpOp {
    fn parse(t: &str) -> Option<CmpOp> {
        match t.trim() {
            "<=" | "=<" => Some(CmpOp::Le),
            ">=" | "=>" => Some(CmpOp::Ge),
            "~" | "*=" => Some(CmpOp::Base(Op::Wild)),
            o => Op::parse(o).map(CmpOp::Base),
        }
    }
    fn test(self, prop: &Value, value: &Value) -> bool {
        match self {
            CmpOp::Base(op) => qselect::compare(prop, op, value),
            CmpOp::Le => qselect::compare(prop, Op::Lt, value) || qselect::compare(prop, Op::Eq, value),
            CmpOp::Ge => qselect::compare(prop, Op::Gt, value) || qselect::compare(prop, Op::Eq, value),
        }
    }
}

fn err(msg: impl Into<String>) -> EngineError {
    bad("filter", msg)
}

// ---------------- the JSON tree ----------------

fn from_json(v: &Value, depth: usize, nodes: &mut usize) -> Result<Cond> {
    *nodes += 1;
    if depth > MAX_DEPTH || *nodes > MAX_NODES {
        return Err(err(format!("a filter holds at most {MAX_NODES} conditions, nested at most {MAX_DEPTH} deep")));
    }
    match v {
        Value::String(t) => parse(t),
        Value::Object(o) => {
            let list = |key: &str, nodes: &mut usize| -> Result<Option<Vec<Cond>>> {
                match o.get(key) {
                    None => Ok(None),
                    Some(Value::Array(a)) if !a.is_empty() => a.iter().map(|x| from_json(x, depth + 1, nodes)).collect::<Result<Vec<_>>>().map(Some),
                    Some(_) => Err(err(format!("`{key}` takes a non-empty list of filters"))),
                }
            };
            if let Some(c) = list("and", nodes)? {
                return Ok(Cond::And(c));
            }
            if let Some(c) = list("or", nodes)? {
                return Ok(Cond::Or(c));
            }
            if let Some(c) = list("xor", nodes)? {
                return Ok(Cond::Xor(c));
            }
            if let Some(n) = o.get("not") {
                return Ok(Cond::Not(Box::new(from_json(n, depth + 1, nodes)?)));
            }
            let property = o
                .get("property")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| err("a condition needs `property`"))?;
            let op = match o.get("op").or_else(|| o.get("operator")) {
                None => CmpOp::Base(Op::Eq),
                Some(Value::String(t)) => CmpOp::parse(t).ok_or_else(|| err(format!("unknown operator `{t}` (use =, !=, <, >, <=, >=, *)")))?,
                Some(x) => return Err(err(format!("`op` must be text, got {x}"))),
            };
            let value = o.get("value").cloned().ok_or_else(|| err(format!("the condition on `{property}` needs `value`")))?;
            Ok(Cond::Test { property: property.to_string(), op, value })
        }
        other => Err(err(format!("a filter is text or an object, got {other}"))),
    }
}

// ---------------- the typed form ----------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Quoted(String),
    Op(String),
    Open,
    Close,
}

fn tokens(t: &str) -> Result<Vec<Tok>> {
    if t.chars().count() > MAX_TEXT {
        return Err(err(format!("a typed filter is at most {MAX_TEXT} characters")));
    }
    let cs: Vec<char> = t.chars().collect();
    let is_op = |i: usize| match cs.get(i) {
        Some(c) if "=!<>~".contains(*c) => true,
        // `*=` is the wildcard operator; a `*` anywhere else belongs to a value (`A*`).
        Some('*') => cs.get(i + 1) == Some(&'='),
        _ => false,
    };
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = cs.get(i) {
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' || c == ')' {
            i += 1;
            out.push(if c == '(' { Tok::Open } else { Tok::Close });
        } else if c == '"' {
            i += 1;
            let mut w = String::new();
            loop {
                match cs.get(i) {
                    Some('"') => break,
                    Some(ch) => w.push(*ch),
                    None => return Err(err("a quoted value is not closed")),
                }
                i += 1;
            }
            i += 1;
            out.push(Tok::Quoted(w));
        } else if is_op(i) {
            let mut o = String::new();
            while o.chars().count() < 2 && (is_op(i) || (!o.is_empty() && cs.get(i) == Some(&'='))) {
                if let Some(ch) = cs.get(i) {
                    o.push(*ch);
                }
                i += 1;
            }
            out.push(Tok::Op(o));
        } else {
            let mut w = String::new();
            while let Some(&ch) = cs.get(i) {
                if ch.is_whitespace() || "()\"".contains(ch) || is_op(i) {
                    break;
                }
                w.push(ch);
                i += 1;
            }
            out.push(Tok::Word(w));
        }
        if out.len() > MAX_NODES * 4 {
            return Err(err("the filter is too long"));
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
    nodes: usize,
}

impl Parser {
    fn peek_word(&self, w: &str) -> bool {
        matches!(self.toks.get(self.at), Some(Tok::Word(x)) if x.eq_ignore_ascii_case(w))
    }
    fn count(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        if self.nodes > MAX_NODES || depth > MAX_DEPTH {
            return Err(err(format!("a filter holds at most {MAX_NODES} conditions, nested at most {MAX_DEPTH} deep")));
        }
        Ok(())
    }
    /// or-expression := xor-expression ("or" xor-expression)*
    fn or(&mut self, depth: usize) -> Result<Cond> {
        let mut v = vec![self.xor(depth)?];
        while self.peek_word("or") || matches!(self.toks.get(self.at), Some(Tok::Word(x)) if x == "|") {
            self.at += 1;
            v.push(self.xor(depth)?);
        }
        Ok(if v.len() == 1 { v.swap_remove(0) } else { Cond::Or(v) })
    }
    fn xor(&mut self, depth: usize) -> Result<Cond> {
        let mut v = vec![self.and(depth)?];
        while self.peek_word("xor") {
            self.at += 1;
            v.push(self.and(depth)?);
        }
        Ok(if v.len() == 1 { v.swap_remove(0) } else { Cond::Xor(v) })
    }
    fn and(&mut self, depth: usize) -> Result<Cond> {
        let mut v = vec![self.unary(depth)?];
        while self.peek_word("and") || matches!(self.toks.get(self.at), Some(Tok::Word(x)) if x == "&") {
            self.at += 1;
            v.push(self.unary(depth)?);
        }
        Ok(if v.len() == 1 { v.swap_remove(0) } else { Cond::And(v) })
    }
    fn unary(&mut self, depth: usize) -> Result<Cond> {
        self.count(depth)?;
        if self.peek_word("not") {
            self.at += 1;
            return Ok(Cond::Not(Box::new(self.unary(depth + 1)?)));
        }
        if self.toks.get(self.at) == Some(&Tok::Open) {
            self.at += 1;
            let c = self.or(depth + 1)?;
            if self.toks.get(self.at) != Some(&Tok::Close) {
                return Err(err("a `(` is not closed"));
            }
            self.at += 1;
            return Ok(c);
        }
        let property = match self.toks.get(self.at) {
            Some(Tok::Word(w) | Tok::Quoted(w)) => w.clone(),
            _ => return Err(err("expected a property name (type, layer, color, …)")),
        };
        self.at += 1;
        let op = match self.toks.get(self.at) {
            Some(Tok::Op(o)) => CmpOp::parse(o).ok_or_else(|| err(format!("unknown operator `{o}` (use =, !=, <, >, <=, >=, ~)")))?,
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("like") => CmpOp::Base(Op::Wild),
            _ => return Err(err(format!("expected an operator after `{property}` (=, !=, <, >, <=, >=, ~)"))),
        };
        self.at += 1;
        let value = match self.toks.get(self.at) {
            Some(Tok::Quoted(w)) => Value::String(w.clone()),
            Some(Tok::Word(w)) => match w.parse::<f64>() {
                Ok(n) if n.is_finite() => json!(n),
                _ => Value::String(w.clone()),
            },
            _ => return Err(err(format!("expected a value after `{property}`"))),
        };
        self.at += 1;
        Ok(Cond::Test { property, op, value })
    }
}

pub(crate) fn parse(t: &str) -> Result<Cond> {
    let toks = tokens(t)?;
    if toks.is_empty() {
        return Err(err("the filter is empty"));
    }
    let mut p = Parser { toks, at: 0, nodes: 0 };
    let c = p.or(0)?;
    if p.at < p.toks.len() {
        return Err(err("unexpected text after the filter (join conditions with and / or)"));
    }
    Ok(c)
}

// ---------------- matching ----------------

/// An object's properties, worked out once and only when a condition needs more than the
/// common ones.
struct Props<'a> {
    d: &'a Drawing,
    e: &'a Entity,
    all: Option<HashMap<String, Value>>,
}

fn norm(k: &str) -> String {
    k.chars().filter(|c| !c.is_whitespace() && *c != '_' && *c != '-').flat_map(char::to_lowercase).collect()
}

impl Props<'_> {
    fn get(&mut self, name: &str) -> Option<Value> {
        let n = norm(name);
        match n.as_str() {
            "type" | "objecttype" | "entity" => return Some(Value::String(self.e.kind.type_name().into())),
            "layer" => return Some(Value::String(self.e.common.layer.clone())),
            "handle" => return Some(Value::String(self.e.handle.hex())),
            _ => {}
        }
        let (d, e) = (self.d, self.e);
        let all = self.all.get_or_insert_with(|| qselect::properties(d, e).into_iter().map(|(k, v)| (norm(&k), v)).collect());
        all.get(&n).cloned()
    }
}

fn type_value_matches(e: &Entity, op: CmpOp, value: &Value) -> Option<bool> {
    // `type = Line` also takes the DXF name (`LWPOLYLINE`) and wildcards.
    let v = value.as_str()?;
    let names = [e.kind.type_name(), e.kind.dxf_name()];
    let hit = names.iter().any(|n| if v.contains(['*', '?']) { qselect::wildcard(v, n) } else { n.eq_ignore_ascii_case(v.trim()) });
    match op {
        CmpOp::Base(Op::Eq | Op::Wild) => Some(hit),
        CmpOp::Base(Op::Ne) => Some(!hit),
        _ => None,
    }
}

fn eval(c: &Cond, p: &mut Props) -> bool {
    match c {
        Cond::Test { property, op, value } => {
            if matches!(norm(property).as_str(), "type" | "objecttype" | "entity")
                && let Some(b) = type_value_matches(p.e, *op, value)
            {
                return b;
            }
            match p.get(property) {
                Some(v) => op.test(&v, value),
                // An object without the property never passes (`!=` included, as QSELECT).
                None => false,
            }
        }
        Cond::And(v) => v.iter().all(|c| eval(c, p)),
        Cond::Or(v) => v.iter().any(|c| eval(c, p)),
        Cond::Xor(v) => v.iter().filter(|c| eval(c, p)).count() % 2 == 1,
        Cond::Not(c) => !eval(c, p),
    }
}

fn run_filter(s: &mut Session, p: &Value) -> Result<Value> {
    let f = p.get("filter").filter(|v| !v.is_null()).ok_or_else(|| err("`filter` is required, e.g. \"type = Circle and radius > 2\""))?;
    let cond = from_json(f, 0, &mut 0)?;
    let mode = str_param(p, "mode").unwrap_or("new").trim().to_ascii_lowercase();
    if !["new", "append", "exclude"].contains(&mode.as_str()) {
        return Err(err(format!("unknown mode `{mode}` (use new, append or exclude)")));
    }
    let apply = str_param(p, "applyTo").unwrap_or("drawing").trim().to_ascii_lowercase();
    let cands: Vec<Handle> = if apply.starts_with("sel") || apply == "current" {
        s.selection()
    } else {
        let space = s.space();
        s.doc()?.space(&space).map(|st| st.iter().take(MAX_SCAN).map(|e| e.handle).collect()).unwrap_or_default()
    };
    let d = s.doc()?;
    let (mut pass, mut fail) = (Vec::new(), Vec::new());
    for h in cands {
        let Some(e) = d.entity(h) else { continue };
        if eval(&cond, &mut Props { d, e, all: None }) {
            pass.push(h);
        } else {
            fail.push(h);
        }
    }
    let sel = match mode.as_str() {
        "exclude" => fail,
        "append" => {
            let mut cur = s.selection();
            let have: std::collections::HashSet<Handle> = cur.iter().copied().collect();
            cur.extend(pass.into_iter().filter(|h| !have.contains(h)));
            cur
        }
        _ => pass,
    };
    s.set_selection(sel);
    let sel = s.selection();
    let n = sel.len();
    Ok(json!({ "selected": n, "handles": sel.iter().map(|h| h.hex()).collect::<Vec<_>>(), "message": format!("{n} object(s) pass the filter.") }))
}

/// FILTER typed: "Enter filter (e.g. type = Line and layer = Walls):" — the objects that pass
/// are selected, for the next command (or its Previous).
struct FilterM;

impl Interactive for FilterM {
    fn name(&self) -> &'static str {
        "FILTER"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Enter filter (e.g. type = Line and layer = Walls)", Accept::TEXT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Text(t) | Input::Keyword(t) if !t.trim().is_empty() => {
                let r = run_filter(s, &json!({ "filter": t }))?;
                let sel = s.selection();
                if !sel.is_empty() {
                    s.remember_selection(&sel);
                }
                super::echo_message(s, &r);
                Ok(Step::Done)
            }
            Input::Enter | Input::Text(_) | Input::Keyword(_) => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
}
