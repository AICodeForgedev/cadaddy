//! Ribbon key tips, as AutoCAD's ribbon shortcuts work: Alt (pressed and released on its own)
//! shows a letter on each ribbon tab and a digit on each Quick Access button; typing a tab's
//! letter opens that tab and shows short tips on its buttons, typing a button's tip runs it.
//! Esc goes back a level; Alt again, a click or a run command hides the tips. While tips show,
//! the letters typed go to them, not to the command line.

use egui::{Color32, Event, Key, Rect, pos2, vec2};

/// Where the key tips are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
    #[default]
    Off,
    /// Letters on the tabs, digits on the Quick Access buttons.
    Tabs,
    /// Tips on the active tab's buttons.
    Buttons,
}

#[derive(Clone, Debug, Default)]
struct State {
    level: Level,
    typed: String,
    /// Alt went down, and nothing else was pressed while it was held.
    alt_down: bool,
    alt_alone: bool,
}

fn id() -> egui::Id {
    egui::Id::new("cadaddy-ribbon-keytips")
}

/// The key tips' level now.
pub fn level(ctx: &egui::Context) -> Level {
    ctx.data(|d| d.get_temp::<State>(id())).map(|s| s.level).unwrap_or_default()
}

/// Hide the key tips (a click elsewhere, a command run).
pub fn hide(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<State>(id()));
}

/// The letter of a ribbon tab (AutoCAD's: Home H, Insert I, Annotate N, Parametric P, View V,
/// Manage M, Output O; others their first letter).
pub fn tab_tip(name: &str) -> String {
    match name {
        "Annotate" => "N".into(),
        n => n.chars().next().map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_default(),
    }
}

/// A tip for each label, unique within the list: its first letter, else its first two letters,
/// else the initials of its first two words, else a letter and a digit.
pub fn assign(labels: &[&str]) -> Vec<String> {
    let mut used: Vec<String> = Vec::new();
    let mut out = Vec::with_capacity(labels.len());
    for (i, label) in labels.iter().enumerate() {
        let letters: Vec<char> = label.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
        let words: Vec<char> = label.split_whitespace().filter_map(|w| w.chars().next()).map(|c| c.to_ascii_uppercase()).collect();
        let first = letters.first().copied().unwrap_or('X');
        let mut candidates: Vec<String> = vec![first.to_string()];
        if let Some(second) = letters.get(1) {
            candidates.push(format!("{first}{second}"));
        }
        if words.len() >= 2 {
            candidates.push(words.iter().take(2).collect());
        }
        candidates.extend((1..=9).map(|d| format!("{first}{d}")));
        candidates.push(format!("Z{i}"));
        // A tip must not be the start of another one, or it would be run before that one's typed.
        let free = |c: &String, used: &[String]| !used.iter().any(|u| u == c || u.starts_with(c.as_str()) || c.starts_with(u.as_str()));
        let tip = candidates.into_iter().find(|c| free(c, &used)).unwrap_or_else(|| format!("Z{i}"));
        used.push(tip.clone());
        out.push(tip);
    }
    out
}

/// What the keyboard asked of the ribbon this frame.
#[derive(Default)]
pub struct Asked {
    pub tab: Option<String>,
    pub cmd: Option<String>,
}

/// Follow the keyboard: Alt alone toggles the tips, letters pick a tab or a button. `tabs` are
/// the ribbon's tab names, `qat` the Quick Access buttons' commands, `buttons` the active tab's
/// (label, command). The keys the tips use are taken out of the input.
pub fn input(ctx: &egui::Context, tabs: &[&str], qat: &[&str], buttons: &[(&str, &str)]) -> Asked {
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    let mut asked = Asked::default();
    let (alt, keys) = ctx.input(|i| (i.modifiers.alt, i.events.iter().any(|e| matches!(e, Event::Key { pressed: true, .. } | Event::Text(_)))));
    // Alt on its own: down, then up with nothing else pressed.
    if alt && !st.alt_down {
        st.alt_down = true;
        st.alt_alone = true;
    } else if alt && keys {
        st.alt_alone = false;
    } else if !alt && st.alt_down {
        st.alt_down = false;
        if st.alt_alone {
            st.level = if st.level == Level::Off { Level::Tabs } else { Level::Off };
            st.typed.clear();
        }
    }
    if st.level != Level::Off {
        let mut typed: Vec<char> = Vec::new();
        let mut escape = false;
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                Event::Text(t) => {
                    typed.extend(t.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()));
                    false
                }
                Event::Key { key: Key::Escape, pressed: true, .. } => {
                    escape = true;
                    false
                }
                Event::Key { pressed: true, key, .. } if key.symbol_or_name().len() == 1 => false,
                Event::PointerButton { pressed: true, .. } => {
                    st.level = Level::Off;
                    true
                }
                _ => true,
            });
        });
        if escape {
            st.level = if st.level == Level::Buttons { Level::Tabs } else { Level::Off };
            st.typed.clear();
        }
        for c in typed {
            match st.level {
                Level::Tabs => {
                    let key = c.to_string();
                    if let Some(tab) = tabs.iter().find(|t| tab_tip(t) == key) {
                        asked.tab = Some((*tab).to_string());
                        st.level = Level::Buttons;
                        st.typed.clear();
                    } else if let Some(cmd) = c.to_digit(10).and_then(|d| qat.get((d as usize).checked_sub(1)?)) {
                        asked.cmd = Some((*cmd).to_string());
                        st.level = Level::Off;
                    }
                }
                Level::Buttons => {
                    st.typed.push(c);
                    let labels: Vec<&str> = buttons.iter().map(|b| b.0).collect();
                    let tips = assign(&labels);
                    if let Some(i) = tips.iter().position(|t| *t == st.typed) {
                        asked.cmd = buttons.get(i).map(|b| b.1.to_string());
                        st.level = Level::Off;
                        st.typed.clear();
                    } else if !tips.iter().any(|t| t.starts_with(st.typed.as_str())) {
                        st.typed.clear();
                    }
                }
                Level::Off => {}
            }
        }
    }
    ctx.data_mut(|d| d.insert_temp(id(), st));
    asked
}

/// A key tip badge below the middle of `r`.
pub fn badge(p: &egui::Painter, r: Rect, tip: &str) {
    let font = egui::FontId::monospace(11.0);
    let g = p.layout_no_wrap(tip.to_string(), font, Color32::BLACK);
    let size = g.size() + vec2(8.0, 4.0);
    let at = Rect::from_center_size(pos2(r.center().x, r.bottom() - size.y / 2.0), size);
    p.rect_filled(at, 3.0, Color32::from_rgb(0xff, 0xf2, 0x9e));
    p.rect_stroke(at, 3.0, egui::Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Inside);
    p.galley(at.min + vec2(4.0, 2.0), g, Color32::BLACK);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tips_are_unique_and_none_starts_another() {
        let labels = ["Line", "Polyline", "Circle", "Arc", "Copy", "Circle", "Line"];
        let tips = assign(&labels);
        assert_eq!(tips[0], "L");
        assert_eq!(tips[1], "P");
        assert_eq!(tips[2], "C");
        assert_eq!(tips[3], "A");
        for (i, a) in tips.iter().enumerate() {
            for (j, b) in tips.iter().enumerate() {
                assert!(i == j || !b.starts_with(a.as_str()), "{a} / {b}");
            }
        }
        assert_eq!(tab_tip("Annotate"), "N");
        assert_eq!(tab_tip("Home"), "H");
    }
}
