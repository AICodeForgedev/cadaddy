//! -PLOT: plotting from the command line, question by question in the order AutoCAD asks them
//! (so plot scripts written for it answer in step), then the same plot as PLOT.
//!
//! Without the detailed configuration: layout, page setup, device, file name, save the page
//! setup, proceed. With it: layout, device, paper, paper units, orientation, upside down, plot
//! area, scale, offset, plot styles and table, lineweights, then shade plot (Model) or the
//! paper-space questions (a layout), file name, save the page setup, proceed.
//!
//! The name questions (layout, device, paper, plot style table, shade plot, file) take the whole line, as
//! names have spaces; their `?` lists the choices.

use cadaddy_doc::PlotOutput;
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::super::plotters;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Longest name or path a typed answer may give.
const MAX_TEXT: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    Detailed,
    Layout,
    PageSetup,
    Device,
    Paper,
    Units,
    Orientation,
    UpsideDown,
    Area,
    ViewName,
    Corner1,
    Corner2,
    Scale,
    Offset,
    PlotStyles,
    StyleTable,
    Lineweights,
    Shade,
    ScaleLineweights,
    PaperSpaceFirst,
    HidePaperSpace,
    File,
    SavePageSetup,
    Proceed,
}

/// Plot areas, as typed.
const MODEL_AREAS: [&str; 5] = ["Display", "Extents", "Limits", "View", "Window"];
const LAYOUT_AREAS: [&str; 5] = ["Display", "Extents", "Layout", "View", "Window"];
const SHADES: [&str; 5] = ["As displayed", "legacy Wireframe", "legacy Hidden", "Visual styles", "Rendered"];

pub(super) struct DashPlotM {
    ask: Ask,
    detailed: bool,
    /// `None`: model space.
    layout: Option<String>,
    device: String,
    paper: String,
    inches: bool,
    landscape: bool,
    upside_down: bool,
    /// Lower case: display, extents, limits, view, window or layout.
    area: String,
    view: Option<String>,
    corner: Option<Vec2>,
    window: Option<[Vec2; 2]>,
    /// Plotted units per drawing unit in the chosen paper units; `None`: fit.
    scale: Option<f64>,
    center: bool,
    /// In the chosen paper units.
    offset: [f64; 2],
    plot_styles: bool,
    style_table: String,
    lineweights: bool,
    shade: &'static str,
    scale_lineweights: bool,
    paper_space_first: bool,
    hide_paper_space: bool,
    path: String,
    save: bool,
}

impl Default for DashPlotM {
    fn default() -> Self {
        DashPlotM {
            ask: Ask::Detailed,
            detailed: false,
            layout: None,
            device: String::new(),
            paper: String::new(),
            inches: false,
            landscape: true,
            upside_down: false,
            area: "display".into(),
            view: None,
            corner: None,
            window: None,
            scale: None,
            center: true,
            offset: [0.0; 2],
            plot_styles: true,
            style_table: String::new(),
            lineweights: true,
            shade: "As displayed",
            scale_lineweights: false,
            paper_space_first: false,
            hide_paper_space: false,
            path: String::new(),
            save: false,
        }
    }
}

fn yes_no(t: &str, default: bool) -> Option<bool> {
    match t.trim().to_ascii_lowercase().as_str() {
        "" => Some(default),
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

fn yn(b: bool) -> &'static str {
    if b { "Yes" } else { "No" }
}

/// The keyword in `list` that `t` names (its whole name or a leading part, any case).
fn keyword(list: &[&'static str], t: &str) -> Option<&'static str> {
    let t = t.trim().to_ascii_lowercase();
    if t.is_empty() {
        return None;
    }
    list.iter().copied().find(|k| k.to_ascii_lowercase() == t).or_else(|| list.iter().copied().find(|k| k.to_ascii_lowercase().starts_with(&t)))
}

/// A typed plot scale, plotted units per drawing unit: `1=48`, `1:50`, `0.25`; `None` when it is
/// not one.
fn typed_scale(t: &str) -> Option<f64> {
    let t = t.trim();
    let k = match t.split_once(['=', ':', '/']) {
        Some((a, b)) => a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok()?,
        None => t.parse::<f64>().ok()?,
    };
    (k.is_finite() && k > 0.0 && k < 1e12).then_some(k)
}

fn typed_point(t: &str) -> Option<Vec2> {
    let (x, y) = t.trim().split_once(',')?;
    let p = Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?);
    (p.x.is_finite() && p.y.is_finite()).then_some(p)
}

fn num(v: f64) -> String {
    format!("{v:.2}")
}

impl DashPlotM {
    fn is_model(&self) -> bool {
        self.layout.is_none()
    }

    fn device_config(&self) -> Option<cadaddy_doc::PlotterConfig> {
        plotters::find_device(&self.device)
    }

    fn extension(&self) -> &'static str {
        match self.device_config().map(|c| c.output) {
            Some(PlotOutput::Png) => "png",
            Some(PlotOutput::Svg) => "svg",
            _ => "pdf",
        }
    }

    /// The plot's file name when none is typed: the drawing's name and the layout's, by the
    /// drawing (Documents for a drawing not saved yet).
    fn default_path(&self, s: &Session) -> String {
        let (dir, stem) = match s.state() {
            Ok(st) => {
                let stem = st.title.rsplit_once('.').map_or(st.title.as_str(), |(a, _)| a).to_string();
                (crate::sysvars::dwg_prefix(st.path.as_deref()), stem)
            }
            Err(_) => (String::new(), "Drawing".into()),
        };
        let tab = self.layout.as_deref().unwrap_or("Model");
        format!("{dir}{stem}-{tab}.{}", self.extension())
    }

    /// Settings from the layout's page setup (a layout) or the plot defaults (Model).
    fn load_page(&mut self, s: &Session) {
        let page = self.layout.as_deref().and_then(|n| s.doc().ok()?.layout(n).map(|l| l.page.clone()));
        let fallback = plotters::devices().into_iter().next().map(|c| c.name).unwrap_or_default();
        match page {
            Some(pg) => {
                self.device = if plotters::find_device(&pg.device).is_some() { pg.device.clone() } else { fallback };
                self.paper = pg.paper.clone();
                self.landscape = pg.landscape;
                self.area = "layout".into();
                self.scale = if pg.scale_to_fit { None } else { Some(1.0) };
                self.center = pg.center;
                self.style_table = pg.plot_style_table.clone();
                self.plot_styles = !pg.plot_style_table.is_empty() && !pg.plot_style_table.eq_ignore_ascii_case("none");
                self.lineweights = pg.lineweights;
                self.inches = s.doc().map(|d| cadaddy_render::paper::page_unit_mm(d, &pg) > 1.0).unwrap_or(false);
            }
            None => {
                self.device = fallback;
                let dev = self.device_config();
                self.paper = dev.as_ref().map(|d| d.default_paper.clone()).unwrap_or_default();
                self.landscape = dev.is_none_or(|d| d.orientation == cadaddy_doc::Orientation::Landscape);
                self.area = "display".into();
                self.scale = None;
                self.center = true;
                self.inches = s.doc().map(|d| cadaddy_render::paper::paper_unit_mm(d) > 1.0).unwrap_or(false);
            }
        }
    }

    /// The plot as PLOT's parameters.
    fn params(&self, s: &Session) -> Value {
        let mut p = json!({
            "layout": self.layout.as_deref().unwrap_or("Model"),
            "device": self.device,
            "landscape": self.landscape,
            "upsideDown": self.upside_down,
            "lineweights": self.lineweights,
            "path": self.path,
        });
        let Some(o) = p.as_object_mut() else { return p };
        if !self.paper.is_empty() {
            o.insert("paper".into(), json!(self.paper));
        }
        o.insert("plotStyleTable".into(), json!(if self.plot_styles && !self.style_table.is_empty() { self.style_table.as_str() } else { "None" }));
        if self.is_model() {
            // Paper units per drawing unit: the typed scale is in the chosen units.
            let user_mm = if self.inches { 25.4 } else { 1.0 };
            let paper_mm = s.doc().map(cadaddy_render::paper::paper_unit_mm).unwrap_or(1.0);
            match self.scale {
                Some(k) => {
                    o.insert("fit".into(), json!(false));
                    o.insert("scale".into(), json!(k * user_mm / paper_mm));
                }
                None => {
                    o.insert("fit".into(), json!(true));
                }
            }
            o.insert("center".into(), json!(self.center));
            if !self.center {
                o.insert("offset".into(), json!([self.offset[0] * user_mm, self.offset[1] * user_mm]));
            }
            o.insert("plotArea".into(), json!(self.area));
            if let Some([a, b]) = self.window {
                o.insert("window".into(), json!([[a.x, a.y], [b.x, b.y]]));
            }
            if let Some(v) = &self.view {
                o.insert("view".into(), json!(v));
            }
        } else {
            o.insert("fit".into(), json!(self.scale.is_none()));
            o.insert("plotArea".into(), json!("layout"));
        }
        if self.save {
            o.insert("savePageSetup".into(), json!(true));
        }
        p
    }

    /// The question after `self.ask` has been answered.
    fn next(&mut self, s: &Session) {
        self.ask = match self.ask {
            Ask::Detailed => Ask::Layout,
            Ask::Layout if self.detailed => Ask::Device,
            Ask::Layout => Ask::PageSetup,
            Ask::PageSetup => Ask::Device,
            Ask::Device if self.detailed => Ask::Paper,
            Ask::Device => self.ask_file(s),
            Ask::Paper => Ask::Units,
            Ask::Units => Ask::Orientation,
            Ask::Orientation => Ask::UpsideDown,
            Ask::UpsideDown => Ask::Area,
            Ask::Area => match self.area.as_str() {
                "view" => Ask::ViewName,
                "window" => Ask::Corner1,
                _ => Ask::Scale,
            },
            Ask::ViewName | Ask::Corner2 => Ask::Scale,
            Ask::Corner1 => Ask::Corner2,
            Ask::Scale => Ask::Offset,
            Ask::Offset => Ask::PlotStyles,
            Ask::PlotStyles if self.plot_styles => Ask::StyleTable,
            Ask::PlotStyles | Ask::StyleTable => Ask::Lineweights,
            Ask::Lineweights if self.is_model() => Ask::Shade,
            Ask::Lineweights => Ask::ScaleLineweights,
            Ask::ScaleLineweights => Ask::PaperSpaceFirst,
            Ask::PaperSpaceFirst => Ask::HidePaperSpace,
            Ask::Shade | Ask::HidePaperSpace => self.ask_file(s),
            Ask::File => Ask::SavePageSetup,
            Ask::SavePageSetup => Ask::Proceed,
            Ask::Proceed => Ask::Proceed,
        };
    }

    fn ask_file(&mut self, s: &Session) -> Ask {
        self.path = self.default_path(s);
        Ask::File
    }

    fn list(&self, s: &mut Session) {
        match self.ask {
            Ask::Layout => {
                let names: Vec<String> = s.doc().map(|d| d.layouts.iter().map(|l| l.name.clone()).collect()).unwrap_or_default();
                s.echo(format!("Layouts: Model, {}", names.join(", ")));
            }
            Ask::Device => s.echo(format!("Output devices: {}", plotters::device_names().join(", "))),
            Ask::Paper => {
                let papers: Vec<String> = self.device_config().map(|c| c.papers.iter().map(|p| p.name.clone()).collect()).unwrap_or_default();
                s.echo(format!("Paper sizes: {}", papers.join(", ")));
            }
            Ask::StyleTable => {
                let mut names: Vec<String> = cadaddy_doc::BUILTIN_PLOT_STYLE_TABLES.iter().map(|n| n.to_string()).collect();
                if let Ok(d) = s.doc() {
                    names.extend(d.plot_style_tables.iter().map(|t| t.name.clone()));
                }
                s.echo(format!("Plot style tables: {}", names.join(", ")));
            }
            _ => {}
        }
    }

    fn proceed(&self, s: &mut Session) -> Result<Step> {
        let p = self.params(s);
        match run_dash_plot(s, &p) {
            Ok(r) => {
                let path = r.get("path").and_then(Value::as_str).unwrap_or(&self.path).to_string();
                s.echo(format!("Plot written to {path}"));
                if self.shade != "As displayed" && self.shade != "legacy Wireframe" {
                    s.echo(format!("Shade plot \"{}\" plots as displayed in this version.", self.shade));
                }
            }
            Err(e) => s.echo(e.to_string()),
        }
        Ok(Step::Done)
    }
}

impl Interactive for DashPlotM {
    fn name(&self) -> &'static str {
        "-PLOT"
    }

    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.layout = s.space().layout().map(str::to_string);
        self.load_page(s);
        Ok(Step::Continue)
    }

    fn prompt(&self, s: &Session) -> Prompt {
        let units = if self.inches { "Inches" } else { "Millimeters" };
        match self.ask {
            Ask::Detailed => Prompt::new("Detailed plot configuration?", Accept::TEXT).kw(&["Yes", "No"]).default("No"),
            Ask::Layout => Prompt::new("Enter a layout name or [?]", Accept::TEXT).default(self.layout.as_deref().unwrap_or("Model")),
            Ask::PageSetup => Prompt::new("Enter a page setup name", Accept::TEXT).default(""),
            Ask::Device => Prompt::new("Enter an output device name or [?]", Accept::TEXT).default(self.device.as_str()),
            Ask::Paper => Prompt::new("Enter paper size or [?]", Accept::TEXT).default(self.paper.as_str()),
            Ask::Units => Prompt::new("Enter paper units [Inches/Millimeters]", Accept::TEXT).kw(&["Inches", "Millimeters"]).default(units),
            Ask::Orientation => Prompt::new("Enter drawing orientation [Portrait/Landscape]", Accept::TEXT)
                .kw(&["Portrait", "Landscape"])
                .default(if self.landscape { "Landscape" } else { "Portrait" }),
            Ask::UpsideDown => Prompt::new("Plot upside down?", Accept::TEXT).kw(&["Yes", "No"]).default(yn(self.upside_down)),
            Ask::Area => {
                let list: &[&str] = if self.is_model() { &MODEL_AREAS } else { &LAYOUT_AREAS };
                let cur = list.iter().find(|k| k.eq_ignore_ascii_case(&self.area)).copied().unwrap_or("Display");
                Prompt::new(format!("Enter plot area [{}]", list.join("/")), Accept::TEXT).kw(list).default(cur)
            }
            Ask::ViewName => Prompt::new("Enter view name", Accept::TEXT),
            Ask::Corner1 => Prompt::new("Enter lower left corner of window", Accept::POINT).default("0.00,0.00"),
            Ask::Corner2 => {
                let p = Prompt::new("Enter upper right corner of window", Accept::POINT).default("0.00,0.00");
                match self.corner {
                    Some(c) => p.corner(c),
                    None => p,
                }
            }
            Ask::Scale => {
                let cur = match self.scale {
                    None => "Fit".to_string(),
                    Some(k) if k >= 1.0 => format!("{}=1", num(k)),
                    Some(k) => format!("1={}", num(1.0 / k)),
                };
                Prompt::new(format!("Enter plot scale (Plotted {units}=Drawing Units)"), Accept::TEXT).kw(&["Fit"]).default(cur)
            }
            Ask::Offset => {
                let cur = format!("{},{}", num(self.offset[0]), num(self.offset[1]));
                let p = Prompt::new("Enter plot offset (x,y)", Accept::TEXT);
                if self.is_model() { p.kw(&["Center"]).default(if self.center { "Center".to_string() } else { cur }) } else { p.default(cur) }
            }
            Ask::PlotStyles => Prompt::new("Plot with plot styles?", Accept::TEXT).kw(&["Yes", "No"]).default(yn(self.plot_styles)),
            Ask::StyleTable => Prompt::new("Enter plot style table name or [?] (enter . for none)", Accept::TEXT).default(self.style_table.as_str()),
            Ask::Lineweights => Prompt::new("Plot with lineweights?", Accept::TEXT).kw(&["Yes", "No"]).default(yn(self.lineweights)),
            Ask::Shade => Prompt::new(format!("Enter shade plot setting [{}]", SHADES.join("/")), Accept::TEXT).default(self.shade),
            Ask::ScaleLineweights => {
                Prompt::new("Scale lineweights with plot scale?", Accept::TEXT).kw(&["Yes", "No"]).default(yn(self.scale_lineweights))
            }
            Ask::PaperSpaceFirst => Prompt::new("Plot paper space first?", Accept::TEXT).kw(&["Yes", "No"]).default(yn(self.paper_space_first)),
            Ask::HidePaperSpace => Prompt::new("Hide paperspace objects?", Accept::TEXT).kw(&["Yes", "No"]).default(yn(self.hide_paper_space)),
            Ask::File => {
                Prompt::new("Enter file name", Accept::TEXT).default(if self.path.is_empty() { self.default_path(s) } else { self.path.clone() })
            }
            Ask::SavePageSetup => Prompt::new("Save changes to page setup?", Accept::TEXT).kw(&["Yes", "No"]).default(yn(self.save)),
            Ask::Proceed => Prompt::new("Proceed with plot?", Accept::TEXT).kw(&["Yes", "No"]).default("Yes"),
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let t = match &i {
            Input::Cancel => return Ok(Step::Cancel),
            Input::Text(t) | Input::Keyword(t) => t.trim().chars().take(MAX_TEXT).collect::<String>(),
            Input::Enter => String::new(),
            Input::Point(p) => match self.ask {
                Ask::Corner1 | Ask::Corner2 => format!("{},{}", p.x, p.y),
                _ => return Ok(Step::Continue),
            },
            _ => return Ok(Step::Continue),
        };
        if t == "?" && matches!(self.ask, Ask::Layout | Ask::Device | Ask::Paper | Ask::StyleTable) {
            self.list(s);
            return Ok(Step::Continue);
        }
        macro_rules! yes_or_retry {
            ($field:expr) => {
                match yes_no(&t, $field) {
                    Some(v) => $field = v,
                    None => {
                        s.echo("Enter Yes or No.");
                        return Ok(Step::Continue);
                    }
                }
            };
        }
        match self.ask {
            Ask::Detailed => yes_or_retry!(self.detailed),
            Ask::Layout => {
                if !t.is_empty() {
                    if t.eq_ignore_ascii_case("model") {
                        self.layout = None;
                    } else {
                        let found = s.doc()?.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(&t)).map(|l| l.name.clone());
                        match found {
                            Some(n) => self.layout = Some(n),
                            None => {
                                s.echo(format!("Layout \"{t}\" not found."));
                                return Ok(Step::Continue);
                            }
                        }
                    }
                }
                self.load_page(s);
            }
            Ask::PageSetup => {
                // A layout's page setup by its name (page setups are kept with their layouts).
                if !t.is_empty() {
                    let page = s.doc()?.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(&t)).map(|l| l.page.clone());
                    match page {
                        Some(pg) => {
                            if plotters::find_device(&pg.device).is_some() {
                                self.device = pg.device.clone();
                            }
                            self.paper = pg.paper.clone();
                            self.landscape = pg.landscape;
                            self.style_table = pg.plot_style_table.clone();
                            self.lineweights = pg.lineweights;
                        }
                        None => {
                            s.echo(format!("Page setup \"{t}\" not found."));
                            return Ok(Step::Continue);
                        }
                    }
                }
            }
            Ask::Device => {
                if !t.is_empty() {
                    match plotters::find_device(&t) {
                        Some(dev) => {
                            if dev.paper(&self.paper).is_none() {
                                self.paper = dev.default_paper.clone();
                            }
                            self.device = dev.name;
                        }
                        None => {
                            s.echo(format!("Device \"{t}\" not found (enter ? for the list)."));
                            return Ok(Step::Continue);
                        }
                    }
                }
            }
            Ask::Paper => {
                if !t.is_empty() {
                    let found = self.device_config().and_then(|c| {
                        let p = c.paper(&t).or_else(|| c.paper(cadaddy_render::paper_size(&t)?.name))?;
                        Some(p.name.clone())
                    });
                    match found {
                        Some(n) => self.paper = n,
                        None => {
                            s.echo(format!("Paper size \"{t}\" not found (enter ? for the list)."));
                            return Ok(Step::Continue);
                        }
                    }
                }
            }
            Ask::Units => match keyword(&["Inches", "Millimeters"], &t) {
                _ if t.is_empty() => {}
                Some(k) => self.inches = k == "Inches",
                None => {
                    s.echo("Enter Inches or Millimeters.");
                    return Ok(Step::Continue);
                }
            },
            Ask::Orientation => match keyword(&["Portrait", "Landscape"], &t) {
                _ if t.is_empty() => {}
                Some(k) => self.landscape = k == "Landscape",
                None => {
                    s.echo("Enter Portrait or Landscape.");
                    return Ok(Step::Continue);
                }
            },
            Ask::UpsideDown => yes_or_retry!(self.upside_down),
            Ask::Area => {
                let list: &[&'static str] = if self.is_model() { &MODEL_AREAS } else { &LAYOUT_AREAS };
                match keyword(list, &t) {
                    _ if t.is_empty() => {}
                    Some(k) if !self.is_model() && k != "Layout" => {
                        s.echo("A layout plots its whole sheet (Layout).");
                        self.area = "layout".into();
                    }
                    Some(k) => self.area = k.to_ascii_lowercase(),
                    None => {
                        s.echo("Invalid option keyword.");
                        return Ok(Step::Continue);
                    }
                }
            }
            Ask::ViewName => {
                let found = s.doc()?.views.iter().find(|v| v.name.eq_ignore_ascii_case(&t)).map(|v| v.name.clone());
                match found {
                    Some(n) => self.view = Some(n),
                    None => {
                        s.echo(format!("View \"{t}\" not found."));
                        return Ok(Step::Continue);
                    }
                }
            }
            Ask::Corner1 | Ask::Corner2 => {
                let p = if t.is_empty() { Some(Vec2::ZERO) } else { typed_point(&t) };
                let Some(p) = p else {
                    s.echo("Enter a point as x,y.");
                    return Ok(Step::Continue);
                };
                match (self.ask, self.corner) {
                    (Ask::Corner2, Some(a)) => self.window = Some([a, p]),
                    _ => self.corner = Some(p),
                }
            }
            Ask::Scale => {
                if t.is_empty() {
                } else if keyword(&["Fit"], &t).is_some() {
                    self.scale = None;
                } else {
                    match typed_scale(&t) {
                        Some(k) => self.scale = Some(k),
                        None => {
                            s.echo("Enter a scale such as 1=48 or 1:50, or Fit.");
                            return Ok(Step::Continue);
                        }
                    }
                }
            }
            Ask::Offset => {
                if t.is_empty() {
                } else if self.is_model() && keyword(&["Center"], &t).is_some() {
                    self.center = true;
                } else {
                    match typed_point(&t).filter(|p| p.x.abs() <= 1e5 && p.y.abs() <= 1e5) {
                        Some(p) => {
                            self.offset = [p.x, p.y];
                            self.center = false;
                            if !self.is_model() && (p.x != 0.0 || p.y != 0.0) {
                                s.echo("Layouts plot at their own sheet origin; the offset applies to Model plots.");
                            }
                        }
                        None => {
                            s.echo("Enter the offset as x,y.");
                            return Ok(Step::Continue);
                        }
                    }
                }
            }
            Ask::PlotStyles => yes_or_retry!(self.plot_styles),
            Ask::StyleTable => {
                if t == "." {
                    self.style_table.clear();
                    self.plot_styles = false;
                } else if !t.is_empty() {
                    let known = cadaddy_doc::BUILTIN_PLOT_STYLE_TABLES.iter().any(|n| n.eq_ignore_ascii_case(&t))
                        || s.doc()?.plot_style_tables.iter().any(|x| x.name.eq_ignore_ascii_case(&t));
                    if !known {
                        s.echo(format!("Plot style table \"{t}\" not found (enter ? for the list)."));
                        return Ok(Step::Continue);
                    }
                    self.style_table = t.clone();
                }
            }
            Ask::Lineweights => yes_or_retry!(self.lineweights),
            Ask::Shade => match keyword(&SHADES, &t) {
                _ if t.is_empty() => {}
                Some(k) => self.shade = k,
                None => {
                    s.echo("Invalid option keyword.");
                    return Ok(Step::Continue);
                }
            },
            Ask::ScaleLineweights => yes_or_retry!(self.scale_lineweights),
            Ask::PaperSpaceFirst => yes_or_retry!(self.paper_space_first),
            Ask::HidePaperSpace => yes_or_retry!(self.hide_paper_space),
            Ask::File => {
                if !t.is_empty() {
                    let t = t.trim_matches('"').to_string();
                    let ext = self.extension();
                    let has_ext = std::path::Path::new(&t).extension().is_some_and(|e| e.eq_ignore_ascii_case(ext));
                    self.path = if has_ext { t } else { format!("{t}.{ext}") };
                }
            }
            Ask::SavePageSetup => yes_or_retry!(self.save),
            Ask::Proceed => {
                let mut go = true;
                yes_or_retry!(go);
                return if go { self.proceed(s) } else { Ok(Step::Done) };
            }
        }
        self.next(s);
        Ok(Step::Continue)
    }
}

/// -PLOT's JSON form: PLOT's parameters, plus `savePageSetup` (store the layout's device, paper,
/// orientation, plot style table, lineweights and fit in its page setup first).
pub(super) fn run_dash_plot(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "-plot";
    if p.get("savePageSetup").and_then(Value::as_bool) == Some(true) {
        let layout = super::str_param(p, "layout")
            .filter(|n| !n.eq_ignore_ascii_case("model"))
            .map(str::to_string)
            .or_else(|| s.space().layout().map(str::to_string));
        match layout {
            Some(name) => {
                let mut setup = json!({ "layout": name });
                if let Some(o) = setup.as_object_mut() {
                    for k in ["device", "paper", "landscape", "lineweights", "plotStyleTable"] {
                        if let Some(v) = p.get(k) {
                            o.insert(k.into(), v.clone());
                        }
                    }
                    if let Some(f) = p.get("fit") {
                        o.insert("scaleToFit".into(), f.clone());
                    }
                }
                super::run_pagesetup(s, &setup)?;
            }
            None => s.echo("Model page setups are not stored yet; the plot uses these settings once."),
        }
    }
    let opts = plotters::plot_options(s, p, "plot")?;
    super::run_plot(s, &opts, cmd)
}
