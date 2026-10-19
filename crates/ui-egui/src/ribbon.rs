//! The ribbon: AutoCAD for Windows' tabbed command bar (Home, Insert, Annotate, …), so someone
//! used to AutoCAD finds every tab, panel and button where their hands expect it. Layout and
//! behaviour follow AutoCAD; every icon is our own (`icons.rs`).
//!
//! Each panel has large buttons, columns of small buttons and special controls (the layer list).
//! A button with a flyout is a split button: the top runs the variant used last, the arrow lists
//! the others. When the window is too narrow, small buttons lose their labels first, then panels
//! collapse from the right into one button that opens the whole panel, as AutoCAD does.

use egui::{Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::CadApp;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

/// A variant in a flyout or a panel's slide-out: (icon, command, label).
pub type Fly = (Icon, &'static str, &'static str);

/// One button: the default variant, plus the flyout (empty for a plain button).
#[derive(Clone, Copy)]
pub struct Btn {
    pub icon: Icon,
    pub cmd: &'static str,
    pub label: &'static str,
    pub flyout: &'static [Fly],
}

const fn b(icon: Icon, cmd: &'static str, label: &'static str) -> Btn {
    Btn { icon, cmd, label, flyout: &[] }
}

const fn f(icon: Icon, cmd: &'static str, label: &'static str, flyout: &'static [Fly]) -> Btn {
    Btn { icon, cmd, label, flyout }
}

pub enum Item {
    /// A large button: 32 px icon, label underneath.
    Large(Btn),
    /// A column of up to three small buttons.
    Smalls(&'static [Btn]),
    /// Layer tools, the current-layer list and Make Current / Match Layer.
    Layers,
}

pub struct Panel {
    pub name: &'static str,
    pub items: &'static [Item],
    /// The panel's slide-out (the arrow beside its title).
    pub more: &'static [Fly],
}

pub struct Tab {
    pub name: &'static str,
    pub panels: &'static [Panel],
}

const CIRCLE: &[Fly] = &[
    (Icon::Circle, "circle", "Center, Radius"),
    (Icon::Circle, "circle.cd", "Center, Diameter"),
    (Icon::Circle, "circle.2p", "2-Point"),
    (Icon::Circle, "circle.3p", "3-Point"),
    (Icon::Circle, "circle.ttr", "Tan, Tan, Radius"),
    (Icon::Circle, "circle.ttt", "Tan, Tan, Tan"),
];
const ARC: &[Fly] = &[
    (Icon::Arc, "arc", "3-Point"),
    (Icon::Arc, "arc.sce", "Start, Center, End"),
    (Icon::Arc, "arc.sca", "Start, Center, Angle"),
    (Icon::Arc, "arc.scl", "Start, Center, Length"),
    (Icon::Arc, "arc.sea", "Start, End, Angle"),
    (Icon::Arc, "arc.sed", "Start, End, Direction"),
    (Icon::Arc, "arc.ser", "Start, End, Radius"),
    (Icon::Arc, "arc.cse", "Center, Start, End"),
    (Icon::Arc, "arc.csa", "Center, Start, Angle"),
    (Icon::Arc, "arc.csl", "Center, Start, Length"),
    (Icon::Arc, "arc.continue", "Continue"),
];
const RECT: &[Fly] = &[(Icon::Rectangle, "rectang", "Rectangle"), (Icon::Polygon, "polygon", "Polygon")];
const ELLIPSE: &[Fly] =
    &[(Icon::Ellipse, "ellipse", "Center"), (Icon::Ellipse, "ellipse.axis", "Axis, End"), (Icon::Ellipse, "ellipse.arc", "Elliptical Arc")];
const HATCH: &[Fly] = &[(Icon::Hatch, "hatch", "Hatch"), (Icon::Gradient, "gradient", "Gradient"), (Icon::Boundary, "boundary", "Boundary")];
const TRIM: &[Fly] = &[(Icon::Trim, "trim", "Trim"), (Icon::Extend, "extend", "Extend")];
const FILLET: &[Fly] = &[(Icon::Fillet, "fillet", "Fillet"), (Icon::Chamfer, "chamfer", "Chamfer"), (Icon::Fillet, "blend", "Blend Curves")];
const ARRAY: &[Fly] = &[
    (Icon::ArrayRect, "arrayrect", "Rectangular Array"),
    (Icon::ArrayPolar, "arraypath", "Path Array"),
    (Icon::ArrayPolar, "arraypolar", "Polar Array"),
];
const TEXT: &[Fly] = &[(Icon::MText, "mtext", "Multiline Text"), (Icon::Text, "text", "Single Line")];
const DIM: &[Fly] = &[
    (Icon::DimLinear, "dimlinear", "Linear"),
    (Icon::DimAligned, "dimaligned", "Aligned"),
    (Icon::DimAngular, "dimangular", "Angular"),
    (Icon::DimArc, "dimarc", "Arc Length"),
    (Icon::DimRadius, "dimradius", "Radius"),
    (Icon::DimDiameter, "dimdiameter", "Diameter"),
    (Icon::DimRadius, "dimjogged", "Jogged"),
    (Icon::DimOrdinate, "dimordinate", "Ordinate"),
];
const LEADER: &[Fly] = &[
    (Icon::MLeader, "mleader", "Multileader"),
    (Icon::AddLeader, "mleaderedit.add", "Add Leader"),
    (Icon::RemoveLeader, "mleaderedit.remove", "Remove Leader"),
    (Icon::AlignLeaders, "mleaderalign", "Align"),
];
const INSERT: &[Fly] = &[(Icon::Insert, "insert", "Insert"), (Icon::Blocks, "ui.dialog.blocks", "Blocks Palette")];
const MEASURE: &[Fly] = &[(Icon::Distance, "dist", "Distance"), (Icon::Area, "area", "Area"), (Icon::Measure, "measuregeom", "Measure")];
const PASTE: &[Fly] =
    &[(Icon::Copy, "pasteclip", "Paste"), (Icon::Insert, "pasteblock", "Paste as Block"), (Icon::Copy, "pasteorig", "Paste to Original Coordinates")];
const DIM_CONTINUE: &[Fly] = &[(Icon::DimContinue, "dimcontinue", "Continue"), (Icon::DimBaseline, "dimbaseline", "Baseline")];
const PLOT: &[Fly] = &[(Icon::Plot, "plot", "Plot")];
const VIEWS: &[Fly] = &[
    (Icon::Workspace, "view.top", "Top"),
    (Icon::Workspace, "view.bottom", "Bottom"),
    (Icon::Workspace, "view.left", "Left"),
    (Icon::Workspace, "view.right", "Right"),
    (Icon::Workspace, "view.front", "Front"),
    (Icon::Workspace, "view.back", "Back"),
    (Icon::Orbit, "view.swiso", "SW Isometric"),
    (Icon::Orbit, "view.seiso", "SE Isometric"),
    (Icon::Orbit, "view.neiso", "NE Isometric"),
    (Icon::Orbit, "view.nwiso", "NW Isometric"),
];

pub const TABS: &[Tab] = &[
    Tab {
        name: "Home",
        panels: &[
            Panel {
                name: "Draw",
                items: &[
                    Item::Large(b(Icon::Line, "line", "Line")),
                    Item::Large(b(Icon::Polyline, "pline", "Polyline")),
                    Item::Large(f(Icon::Circle, "circle", "Circle", CIRCLE)),
                    Item::Large(f(Icon::Arc, "arc", "Arc", ARC)),
                    Item::Smalls(&[
                        f(Icon::Rectangle, "rectang", "", RECT),
                        f(Icon::Hatch, "hatch", "", HATCH),
                        f(Icon::Ellipse, "ellipse", "", ELLIPSE),
                    ]),
                ],
                more: &[
                    (Icon::XLine, "xline", "Construction Line"),
                    (Icon::Ray, "ray", "Ray"),
                    (Icon::Point, "point.multiple", "Multiple Points"),
                    (Icon::Spline, "spline", "Spline Fit"),
                    (Icon::Spline, "spline.cv", "Spline CV"),
                    (Icon::Region, "region", "Region"),
                    (Icon::Wipeout, "wipeout", "Wipeout"),
                    (Icon::RevCloud, "revcloud", "Revision Cloud"),
                    (Icon::Donut, "donut", "Donut"),
                    (Icon::Point, "divide", "Divide"),
                    (Icon::Point, "measure", "Measure"),
                ],
            },
            Panel {
                name: "Modify",
                items: &[
                    Item::Smalls(&[b(Icon::Move, "move", "Move"), b(Icon::Copy, "copy", "Copy"), b(Icon::Stretch, "stretch", "Stretch")]),
                    Item::Smalls(&[b(Icon::Rotate, "rotate", "Rotate"), b(Icon::Mirror, "mirror", "Mirror"), b(Icon::Scale, "scale", "Scale")]),
                    Item::Smalls(&[
                        f(Icon::Trim, "trim", "Trim", TRIM),
                        f(Icon::Fillet, "fillet", "Fillet", FILLET),
                        f(Icon::ArrayRect, "arrayrect", "Array", ARRAY),
                    ]),
                    Item::Smalls(&[b(Icon::Erase, "erase", ""), b(Icon::Explode, "explode", ""), b(Icon::Offset, "offset", "")]),
                ],
                more: &[
                    (Icon::Lengthen, "lengthen", "Lengthen"),
                    (Icon::Break, "breakatpoint", "Break at Point"),
                    (Icon::Break, "break", "Break"),
                    (Icon::Join, "join", "Join"),
                    (Icon::Chamfer, "chamfer", "Chamfer"),
                    (Icon::Fillet, "blend", "Blend Curves"),
                    (Icon::Polyline, "pedit", "Edit Polyline"),
                    (Icon::Spline, "splinedit", "Edit Spline"),
                    (Icon::Hatch, "hatchedit", "Edit Hatch"),
                    (Icon::Move, "align", "Align"),
                    (Icon::Polyline, "reverse", "Reverse"),
                    (Icon::ArrayRect, "draworder", "Draw Order"),
                    (Icon::Text, "texttofront", "Bring Text to Front"),
                    (Icon::Hatch, "hatchtoback", "Send Hatches to Back"),
                    (Icon::Erase, "overkill", "Delete Duplicate Objects"),
                ],
            },
            Panel {
                name: "Annotation",
                items: &[
                    Item::Large(f(Icon::MText, "mtext", "Text", TEXT)),
                    Item::Large(f(Icon::DimLinear, "dimlinear", "Dimension", DIM)),
                    Item::Smalls(&[f(Icon::MLeader, "mleader", "Leader", LEADER), b(Icon::Table, "table", "Table")]),
                ],
                more: &[
                    (Icon::Text, "style", "Text Style"),
                    (Icon::DimLinear, "dimstyle", "Dimension Style"),
                    (Icon::MLeader, "mleaderstyle", "Multileader Style"),
                    (Icon::Table, "tablestyle", "Table Style"),
                ],
            },
            Panel {
                name: "Layers",
                items: &[Item::Large(b(Icon::LayerProps, "ui.dialog.layers", "Layer Properties")), Item::Layers],
                more: &[
                    (Icon::LayerIso, "layuniso", "Unisolate"),
                    (Icon::Bulb, "layon", "Turn All Layers On"),
                    (Icon::Sun, "laythw", "Thaw All Layers"),
                    (Icon::Unlock, "layulk", "Unlock"),
                    (Icon::LayerPrev, "layerp", "Previous"),
                    (Icon::Layers, "laymrg", "Merge"),
                    (Icon::Erase, "laydel", "Delete"),
                    (Icon::MakeCurrent, "laycur", "Change to Current Layer"),
                ],
            },
            Panel {
                name: "Block",
                items: &[
                    Item::Large(f(Icon::Insert, "insert", "Insert", INSERT)),
                    Item::Smalls(&[
                        b(Icon::BlockCreate, "block", "Create"),
                        b(Icon::BlockEdit, "bedit", "Edit"),
                        b(Icon::AttDef, "attedit", "Edit Attributes"),
                    ]),
                ],
                more: &[
                    (Icon::AttDef, "attdef", "Define Attributes"),
                    (Icon::AttDef, "battman", "Manage Attributes"),
                    (Icon::BlockCreate, "wblock", "Write Block"),
                ],
            },
            Panel {
                name: "Properties",
                items: &[
                    Item::Large(b(Icon::MatchProp, "matchprop", "Match Properties")),
                    Item::Smalls(&[b(Icon::Properties, "ui.panel.properties", "Properties"), b(Icon::List, "list", "List")]),
                ],
                more: &[(Icon::Properties, "linetype", "Linetype"), (Icon::Lineweight, "units", "Units")],
            },
            Panel {
                name: "Groups",
                items: &[
                    Item::Large(b(Icon::Blocks, "group", "Group")),
                    Item::Smalls(&[b(Icon::Blocks, "ungroup", "Ungroup"), b(Icon::BlockEdit, "groupedit", "Group Edit")]),
                ],
                more: &[],
            },
            Panel {
                name: "Utilities",
                items: &[
                    Item::Large(f(Icon::Measure, "dist", "Measure", MEASURE)),
                    Item::Smalls(&[
                        b(Icon::QuickSelect, "qselect", "Quick Select"),
                        b(Icon::SelectObjects, "selectall", "Select All"),
                        b(Icon::Point, "id", "ID Point"),
                    ]),
                ],
                more: &[],
            },
            Panel {
                name: "Clipboard",
                items: &[
                    Item::Large(f(Icon::Copy, "pasteclip", "Paste", PASTE)),
                    Item::Smalls(&[b(Icon::Erase, "cutclip", "Cut"), b(Icon::Copy, "copyclip", "Copy Clip")]),
                ],
                more: &[],
            },
        ],
    },
    Tab {
        name: "Insert",
        panels: &[
            Panel {
                name: "Block",
                items: &[
                    Item::Large(f(Icon::Insert, "insert", "Insert", INSERT)),
                    Item::Large(b(Icon::Blocks, "ui.panel.symbols", "Symbols")),
                    Item::Large(b(Icon::AttDef, "attedit", "Edit Attribute")),
                ],
                more: &[],
            },
            Panel {
                name: "Block Definition",
                items: &[
                    Item::Large(b(Icon::BlockCreate, "block", "Create Block")),
                    Item::Large(b(Icon::AttDef, "attdef", "Define Attributes")),
                    Item::Smalls(&[
                        b(Icon::AttDef, "battman", "Manage Attributes"),
                        b(Icon::BlockEdit, "bedit", "Block Editor"),
                        b(Icon::BlockCreate, "wblock", "Write Block"),
                    ]),
                ],
                more: &[],
            },
            Panel {
                name: "Reference",
                items: &[Item::Large(b(Icon::Attach, "xattach", "Attach")), Item::Smalls(&[b(Icon::Attach, "xref", "External References")])],
                more: &[],
            },
            Panel {
                name: "Import",
                items: &[
                    Item::Large(b(Icon::Import, "ui.pdfimport", "PDF Import")),
                    Item::Smalls(&[b(Icon::Import, "import", "Import"), b(Icon::Export, "ui.convertpdf", "PDF to DWG")]),
                ],
                more: &[],
            },
            Panel {
                name: "Data",
                items: &[
                    Item::Large(b(Icon::Table, "dataextraction", "Extract Data")),
                    Item::Smalls(&[b(Icon::Table, "eattext", "Attribute Extraction")]),
                ],
                more: &[],
            },
        ],
    },
    Tab {
        name: "Annotate",
        panels: &[
            Panel {
                name: "Text",
                items: &[
                    Item::Large(f(Icon::MText, "mtext", "Multiline Text", TEXT)),
                    Item::Smalls(&[b(Icon::Search, "find", "Find Text"), b(Icon::Text, "style", "Text Style")]),
                ],
                more: &[],
            },
            Panel {
                name: "Dimensions",
                items: &[
                    Item::Large(b(Icon::DimLinear, "dim", "Dimension")),
                    Item::Large(f(Icon::DimLinear, "dimlinear", "Linear", DIM)),
                    Item::Smalls(&[
                        b(Icon::QDim, "qdim", "Quick"),
                        f(Icon::DimContinue, "dimcontinue", "Continue", DIM_CONTINUE),
                        b(Icon::DimLinear, "dimstyle", "Dimension Style"),
                    ]),
                    Item::Smalls(&[
                        b(Icon::DimLinear, "dimbreak", "Break"),
                        b(Icon::DimLinear, "dimspace", "Adjust Space"),
                        b(Icon::DimLinear, "diminspect", "Inspect"),
                    ]),
                ],
                more: &[
                    (Icon::DimLinear, "dimedit.oblique", "Oblique"),
                    (Icon::DimLinear, "dimtedit", "Text Angle"),
                    (Icon::DimLinear, "dimreassociate", "Reassociate"),
                ],
            },
            Panel {
                name: "Centerlines",
                items: &[Item::Large(b(Icon::Circle, "centermark", "Center Mark")), Item::Large(b(Icon::Line, "centerline", "Centerline"))],
                more: &[],
            },
            Panel {
                name: "Leaders",
                items: &[
                    Item::Large(b(Icon::MLeader, "mleader", "Multileader")),
                    Item::Smalls(&[
                        b(Icon::AddLeader, "mleaderedit.add", "Add Leader"),
                        b(Icon::RemoveLeader, "mleaderedit.remove", "Remove Leader"),
                        b(Icon::AlignLeaders, "mleaderalign", "Align"),
                    ]),
                ],
                more: &[(Icon::MLeader, "mleadercollect", "Collect"), (Icon::MLeader, "mleaderstyle", "Multileader Style")],
            },
            Panel {
                name: "Tables",
                items: &[
                    Item::Large(b(Icon::Table, "table", "Table")),
                    Item::Smalls(&[b(Icon::Table, "dataextraction", "Extract Data"), b(Icon::Table, "tablestyle", "Table Style")]),
                ],
                more: &[],
            },
            Panel {
                name: "Markup",
                items: &[Item::Large(b(Icon::RevCloud, "revcloud", "Revision Cloud")), Item::Large(b(Icon::Wipeout, "wipeout", "Wipeout"))],
                more: &[],
            },
            Panel { name: "Annotation Scaling", items: &[Item::Large(b(Icon::Annotation, "scalelistedit", "Scale List"))], more: &[] },
        ],
    },
    Tab {
        name: "Parametric",
        panels: &[
            Panel {
                name: "Geometric",
                items: &[
                    Item::Large(b(Icon::Constraint, "autoconstrain", "Auto Constrain")),
                    Item::Smalls(&[
                        b(Icon::Coincident, "gccoincident", ""),
                        b(Icon::Parallel, "gcparallel", ""),
                        b(Icon::Perpendicular, "gcperpendicular", ""),
                    ]),
                    Item::Smalls(&[b(Icon::Horizontal, "gchorizontal", ""), b(Icon::Vertical, "gcvertical", ""), b(Icon::Tangent, "gctangent", "")]),
                    Item::Smalls(&[b(Icon::Concentric, "gcconcentric", ""), b(Icon::Equal, "gcequal", ""), b(Icon::Fix, "gcfix", "")]),
                    Item::Smalls(&[b(Icon::Line, "gccollinear", ""), b(Icon::Spline, "gcsmooth", ""), b(Icon::Mirror, "gcsymmetric", "")]),
                ],
                more: &[],
            },
            Panel {
                name: "Dimensional",
                items: &[
                    Item::Large(b(Icon::DimLinear, "dclinear", "Linear")),
                    Item::Large(b(Icon::DimAligned, "dcaligned", "Aligned")),
                    Item::Smalls(&[
                        b(Icon::DimRadius, "dcradius", "Radius"),
                        b(Icon::DimDiameter, "dcdiameter", "Diameter"),
                        b(Icon::DimAngular, "dcangular", "Angular"),
                    ]),
                ],
                more: &[],
            },
            Panel {
                name: "Manage",
                items: &[
                    Item::Large(b(Icon::Erase, "delconstraint", "Delete Constraints")),
                    Item::Large(b(Icon::List, "parameters", "Parameters Manager")),
                ],
                more: &[],
            },
        ],
    },
    Tab {
        name: "View",
        panels: &[
            Panel { name: "Walk", items: &[Item::Large(b(Icon::Walk, "ui.walk", "Walk"))], more: &[] },
            Panel {
                name: "Viewport Tools",
                items: &[Item::Large(b(Icon::Grid, "ui.toggle.ucsicon", "UCS Icon")), Item::Large(b(Icon::Orbit, "ui.toggle.viewcube", "View Cube"))],
                more: &[],
            },
            Panel {
                name: "Named Views",
                items: &[
                    Item::Large(f(Icon::Orbit, "view.swiso", "Views", VIEWS)),
                    Item::Smalls(&[b(Icon::Workspace, "plan", "Plan View"), b(Icon::List, "view", "View Manager")]),
                ],
                more: &[],
            },
            Panel {
                name: "Model Viewports",
                items: &[
                    Item::Large(b(Icon::Workspace, "vports", "Viewport Configuration")),
                    Item::Smalls(&[b(Icon::Workspace, "vports.1", "Single"), b(Icon::Workspace, "vports.2", "Two")]),
                ],
                more: &[],
            },
            Panel {
                name: "Navigate",
                items: &[
                    Item::Large(b(Icon::ZoomExtents, "zoom.extents", "Extents")),
                    Item::Smalls(&[
                        b(Icon::ZoomWindow, "zoom.window", "Window"),
                        b(Icon::ZoomWindow, "zoom.previous", "Previous"),
                        b(Icon::Pan, "pan", "Pan"),
                    ]),
                ],
                more: &[],
            },
            Panel {
                name: "Palettes",
                items: &[
                    Item::Large(b(Icon::Properties, "ui.panel.properties", "Properties")),
                    Item::Large(b(Icon::Layers, "ui.dialog.layers", "Layer Properties")),
                    Item::Smalls(&[
                        b(Icon::Workspace, "ui.toggle.toolsets", "Tool Sets"),
                        b(Icon::Blocks, "ui.dialog.blocks", "Blocks"),
                        b(Icon::Measure, "ui.panel.quickcalc", "QuickCalc"),
                    ]),
                ],
                more: &[],
            },
            Panel {
                name: "Interface",
                items: &[
                    Item::Large(b(Icon::Switcher, "ui.toggle.filetabs", "File Tabs")),
                    Item::Smalls(&[b(Icon::Menu, "ui.toggle.statusbar", "Status Bar"), b(Icon::Menu, "ui.toggle.cmdline", "Command Line")]),
                ],
                more: &[],
            },
        ],
    },
    Tab {
        name: "Manage",
        panels: &[
            Panel {
                name: "Applications",
                items: &[Item::Large(b(Icon::Open, "appload", "Load Application")), Item::Large(b(Icon::List, "script", "Run Script"))],
                more: &[],
            },
            Panel {
                name: "Cleanup",
                items: &[
                    Item::Large(b(Icon::Erase, "purge", "Purge")),
                    Item::Smalls(&[b(Icon::Erase, "overkill", "Overkill"), b(Icon::Search, "audit", "Audit")]),
                ],
                more: &[],
            },
            Panel { name: "Customization", items: &[Item::Large(b(Icon::Gear, "aliasedit", "Edit Aliases"))], more: &[] },
        ],
    },
    Tab {
        name: "Output",
        panels: &[
            Panel {
                name: "Plot",
                items: &[
                    Item::Large(f(Icon::Plot, "plot", "Plot", PLOT)),
                    Item::Large(b(Icon::Publish, "publish", "Batch Plot")),
                    Item::Smalls(&[
                        b(Icon::PageSetup, "pagesetup", "Page Setup Manager"),
                        b(Icon::Preview, "preview", "Preview"),
                        b(Icon::Plot, "plottermanager", "Plotter Manager"),
                    ]),
                ],
                more: &[],
            },
            Panel {
                name: "Export to DWF/PDF",
                items: &[
                    Item::Large(b(Icon::Export, "exportpdf", "Export PDF")),
                    Item::Smalls(&[b(Icon::Export, "export", "Export"), b(Icon::Publish, "pdfbook", "PDF Book")]),
                ],
                more: &[],
            },
        ],
    },
];

/// Quick Access Toolbar, as AutoCAD's default.
const QAT: &[(Icon, &str, &str)] = &[
    (Icon::New, "new", "New"),
    (Icon::Open, "ui.open", "Open"),
    (Icon::Save, "qsave", "Save"),
    (Icon::SaveAs, "ui.saveas", "Save As"),
    (Icon::Plot, "plot", "Plot"),
    (Icon::Undo, "u", "Undo"),
    (Icon::Redo, "redo", "Redo"),
];

/// The application menu (our own button at the left of the title row).
const APP_MENU: &[(Icon, &str, &str)] = &[
    (Icon::New, "new", "New"),
    (Icon::Open, "ui.open", "Open"),
    (Icon::Import, "ui.convertpdf", "Convert PDF to DWG..."),
    (Icon::Save, "qsave", "Save"),
    (Icon::SaveAs, "ui.saveas", "Save As"),
    (Icon::Import, "import", "Import"),
    (Icon::Export, "export", "Export"),
    (Icon::Publish, "publish", "Publish"),
    (Icon::Plot, "plot", "Print"),
    (Icon::Search, "audit", "Drawing Utilities: Audit"),
    (Icon::Erase, "purge", "Drawing Utilities: Purge"),
    (Icon::Lineweight, "units", "Drawing Utilities: Units"),
    (Icon::Gear, "options", "Options"),
    (Icon::Menu, "ui.toggle.menubar", "Show Menu Bar"),
    (Icon::Close, "close", "Close"),
    (Icon::Close, "closeall", "Close All"),
    (Icon::Close, "ui.quit", "Quit CADaddy"),
];

pub const TITLE_H: f32 = 30.0;
pub const TAB_H: f32 = 24.0;
pub const BODY_H: f32 = 102.0;
const PANEL_TITLE_H: f32 = 20.0;
const ROW_H: f32 = 23.0;
const PAD: f32 = 5.0;
const COLLAPSED_W: f32 = 66.0;
const LAYER_W: f32 = 196.0;

/// The ribbon's height below the title row.
pub fn height() -> f32 {
    TAB_H + BODY_H
}

#[derive(Clone, Copy, PartialEq)]
enum Fit {
    /// Small buttons show their labels.
    Full,
    /// Small buttons are icons only.
    Compact,
}

fn label_w(ui: &egui::Ui, s: &str, font: egui::FontId) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    ui.painter().layout_no_wrap(crate::i18n::t(s).to_string(), font, egui::Color32::WHITE).size().x
}

/// A large button's label in at most two lines (AutoCAD wraps "Layer Properties").
fn large_lines(label: &str) -> (String, Option<String>) {
    let s = crate::i18n::t(label);
    if s.chars().count() <= 9 {
        return (s.to_string(), None);
    }
    match s.rfind(' ') {
        Some(i) => (s[..i].to_string(), Some(s[i + 1..].to_string())),
        None => (s.to_string(), None),
    }
}

fn item_w(ui: &egui::Ui, item: &Item, fit: Fit) -> f32 {
    match item {
        Item::Large(btn) => {
            let (a, bb) = large_lines(btn.label);
            let w = label_w(ui, &a, crate::theme::small()).max(bb.map_or(0.0, |s| label_w(ui, &s, crate::theme::small())));
            (w + 10.0).max(46.0)
        }
        Item::Smalls(btns) => {
            let widest = btns
                .iter()
                .map(|bt| {
                    let lw = if fit == Fit::Full { label_w(ui, bt.label, crate::theme::small()) } else { 0.0 };
                    20.0 + if lw > 0.0 { lw + 6.0 } else { 0.0 } + if bt.flyout.is_empty() { 0.0 } else { 11.0 }
                })
                .fold(0.0, f32::max);
            widest + 4.0
        }
        Item::Layers => LAYER_W,
    }
}

fn panel_w(ui: &egui::Ui, panel: &Panel, fit: Fit) -> f32 {
    let items: f32 = panel.items.iter().map(|i| item_w(ui, i, fit)).sum::<f32>() + 2.0 * (panel.items.len().saturating_sub(1)) as f32;
    let title = label_w(ui, panel.name, crate::theme::small()) + if panel.more.is_empty() { 0.0 } else { 14.0 };
    items.max(title) + 2.0 * PAD
}

/// What a click asked for, run once the ribbon is drawn.
#[derive(Default)]
struct Clicks {
    cmd: Option<String>,
    /// (button's default command, the variant chosen): remembered so the split button shows it.
    recent: Option<(String, String)>,
    layer_current: Option<String>,
    layer_toggle: Option<(String, &'static str, bool)>,
    tab: Option<String>,
    /// The active tab's buttons as drawn (rectangle, label, command), for the key tips.
    buttons: Vec<(Rect, &'static str, &'static str)>,
    /// The tab row's and the Quick Access buttons' rectangles (name or command).
    tab_rects: Vec<(Rect, &'static str)>,
    qat_rects: Vec<Rect>,
}

fn recent_variant(app: &CadApp, btn: &Btn) -> Btn {
    let Some((_, cmd)) = app.ui.ribbon_recent.iter().find(|(k, _)| k == btn.cmd) else { return *btn };
    match btn.flyout.iter().find(|(_, c, _)| c == cmd) {
        // Keep the panel's own label (AutoCAD shows the variant's name, which is often longer
        // than the button's; the tooltip names the variant).
        Some((icon, c, _)) => Btn { icon: *icon, cmd: c, label: btn.label, flyout: btn.flyout },
        None => *btn,
    }
}

fn variant_label(btn: &Btn) -> &'static str {
    btn.flyout.iter().find(|(_, c, _)| *c == btn.cmd).map_or(btn.label, |(_, _, l)| *l)
}

fn command_known(id: &str) -> bool {
    cadaddy_engine::find_command(id).is_some() || crate::menus::is_ui_command(id) || id.starts_with("ui.dialog.")
}

/// The flyout list under a split button's arrow.
fn flyout_menu(resp: &egui::Response, key: &'static str, list: &'static [Fly], clicks: &mut Clicks) {
    egui::Popup::from_toggle_button_response(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
        ui.set_min_width(180.0);
        for (icon, cmd, label) in list.iter().filter(|(_, c, _)| command_known(c)) {
            let (r, row) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
            if row.hovered() {
                ui.painter().rect_filled(r, 3.0, Tokens::get().control_hover);
            }
            icons::paint(ui.painter(), Rect::from_min_size(pos2(r.left() + 4.0, r.top() + 3.0), vec2(18.0, 18.0)), *icon, false);
            ui.painter().text(
                pos2(r.left() + 28.0, r.center().y),
                egui::Align2::LEFT_CENTER,
                crate::i18n::t(label),
                crate::theme::body(),
                Tokens::get().text,
            );
            icons::describe_control(ui, &row, label, None, false);
            if row.clicked() {
                clicks.cmd = Some((*cmd).to_string());
                if !key.is_empty() {
                    clicks.recent = Some((key.to_string(), (*cmd).to_string()));
                }
            }
        }
    });
}

fn large_button(app: &CadApp, ui: &mut egui::Ui, rect: Rect, btn: &Btn, clicks: &mut Clicks) {
    let t = Tokens::get();
    let shown = recent_variant(app, btn);
    let enabled = command_known(shown.cmd);
    let split = !btn.flyout.is_empty();
    let top = if split { Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 36.0)) } else { rect };
    let resp = ui.interact(top, ui.id().with(("rb_l", btn.cmd, rect.left() as i32)), Sense::click());
    if resp.hovered() && enabled {
        ui.painter().rect_filled(top, 3.0, t.control_hover.gamma_multiply(0.6));
    }
    icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.center().x, rect.top() + 18.0), vec2(30.0, 30.0)), shown.icon, !enabled);
    let (l1, l2) = large_lines(btn.label);
    let col = if enabled { t.text } else { t.text_faint };
    ui.painter().text(pos2(rect.center().x, rect.top() + 36.0), egui::Align2::CENTER_TOP, &l1, crate::theme::small(), col);
    let mut y = rect.top() + 49.0;
    if let Some(l2) = &l2 {
        ui.painter().text(pos2(rect.center().x, y), egui::Align2::CENTER_TOP, l2, crate::theme::small(), col);
        y += 13.0;
    }
    let tip = if split { variant_label(&shown) } else { btn.label };
    icons::describe_control(ui, &resp, tip, None, false);
    if enabled {
        clicks.buttons.push((top, tip, shown.cmd));
    }
    if resp.on_hover_text(crate::i18n::t(tip)).clicked() && enabled {
        clicks.cmd = Some(shown.cmd.to_string());
    }
    if split {
        let ar = Rect::from_min_max(pos2(rect.left(), rect.top() + 36.0), rect.max);
        let aresp = ui.interact(ar, ui.id().with(("rb_la", btn.cmd, rect.left() as i32)), Sense::click());
        if aresp.hovered() {
            ui.painter().rect_filled(ar, 3.0, t.control_hover.gamma_multiply(0.4));
        }
        icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.center().x, y + 4.0), vec2(9.0, 9.0)), Icon::ChevronDown, false);
        icons::describe_control(ui, &aresp, &format!("{} options", btn.label), None, false);
        flyout_menu(&aresp, btn.cmd, btn.flyout, clicks);
    }
}

fn small_button(app: &CadApp, ui: &mut egui::Ui, rect: Rect, btn: &Btn, fit: Fit, clicks: &mut Clicks) {
    let t = Tokens::get();
    let shown = recent_variant(app, btn);
    let enabled = command_known(shown.cmd);
    let split = !btn.flyout.is_empty();
    let main = if split { Rect::from_min_max(rect.min, pos2(rect.right() - 11.0, rect.bottom())) } else { rect };
    let resp = ui.interact(main, ui.id().with(("rb_s", btn.cmd, rect.left() as i32, rect.top() as i32)), Sense::click());
    if resp.hovered() && enabled {
        ui.painter().rect_filled(main, 3.0, t.control_hover.gamma_multiply(0.6));
    }
    icons::paint(ui.painter(), Rect::from_min_size(pos2(rect.left() + 2.0, rect.center().y - 9.0), vec2(18.0, 18.0)), shown.icon, !enabled);
    if fit == Fit::Full && !btn.label.is_empty() {
        let col = if enabled { t.text } else { t.text_faint };
        ui.painter().text(
            pos2(rect.left() + 24.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            crate::i18n::t(btn.label),
            crate::theme::small(),
            col,
        );
    }
    let tip = if split {
        variant_label(&shown)
    } else if btn.label.is_empty() {
        cadaddy_engine::find_command(btn.cmd).map_or(btn.cmd, |c| c.label)
    } else {
        btn.label
    };
    icons::describe_control(ui, &resp, tip, None, false);
    if enabled {
        clicks.buttons.push((main, tip, shown.cmd));
    }
    if resp.on_hover_text(crate::i18n::t(tip)).clicked() && enabled {
        clicks.cmd = Some(shown.cmd.to_string());
    }
    if split {
        let ar = Rect::from_min_max(pos2(rect.right() - 11.0, rect.top()), rect.max);
        let aresp = ui.interact(ar, ui.id().with(("rb_sa", btn.cmd, rect.left() as i32, rect.top() as i32)), Sense::click());
        if aresp.hovered() {
            ui.painter().rect_filled(ar, 3.0, t.control_hover.gamma_multiply(0.4));
        }
        icons::paint(ui.painter(), Rect::from_center_size(ar.center(), vec2(8.0, 8.0)), Icon::ChevronDown, false);
        icons::describe_control(ui, &aresp, &format!("{} options", variant_label(btn)), None, false);
        flyout_menu(&aresp, btn.cmd, btn.flyout, clicks);
    }
}

/// The Layers panel's middle: layer tools, the current-layer list, Make Current / Match Layer.
fn layer_controls(app: &CadApp, ui: &mut egui::Ui, rect: Rect, clicks: &mut Clicks) {
    let t = Tokens::get();
    let mut x = rect.left();
    for (icon, cmd, tip) in [
        (Icon::LayerOff, "layoff", "Off"),
        (Icon::LayerIso, "layiso", "Isolate"),
        (Icon::LayerFreeze, "layfrz", "Freeze"),
        (Icon::LayerLock, "laylck", "Lock"),
        (Icon::LayerPrev, "layerp", "Previous"),
    ] {
        let br = Rect::from_min_size(pos2(x, rect.top()), vec2(ROW_H, ROW_H));
        let resp = ui.interact(br, ui.id().with(("rb_lay", cmd)), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(br, 3.0, t.control_hover.gamma_multiply(0.6));
        }
        icons::paint(ui.painter(), br.shrink(3.0), icon, false);
        icons::describe_control(ui, &resp, tip, None, false);
        if resp.on_hover_text(crate::i18n::t(tip)).clicked() {
            clicks.cmd = Some(cmd.to_string());
        }
        x += ROW_H + 2.0;
    }
    // Current layer list.
    let combo = Rect::from_min_size(pos2(rect.left(), rect.top() + ROW_H + 1.0), vec2(rect.width(), ROW_H));
    let Ok(d) = app.session.doc() else { return };
    let cur = d.header.str("CLAYER", "0");
    let layers: Vec<_> = d.layers.iter().map(|l| (l.name.clone(), l.on, l.frozen, l.locked, l.color)).collect();
    let resp = ui.interact(combo, ui.id().with("rb_layer_combo"), Sense::click());
    let p = ui.painter();
    p.rect_filled(combo, 3.0, t.control);
    if let Some((name, on, frozen, locked, color)) = layers.iter().find(|l| l.0.eq_ignore_ascii_case(&cur)) {
        let mut lx = combo.left() + 4.0;
        for icon in [
            if *on { Icon::Bulb } else { Icon::BulbOff },
            if *frozen { Icon::Snowflake } else { Icon::Sun },
            if *locked { Icon::Lock } else { Icon::Unlock },
        ] {
            icons::paint(p, Rect::from_min_size(pos2(lx, combo.top() + 4.0), vec2(15.0, 15.0)), icon, false);
            lx += 17.0;
        }
        crate::palettes::swatch(p, Rect::from_min_size(pos2(lx + 1.0, combo.top() + 7.0), vec2(9.0, 9.0)), *color);
        p.text(pos2(lx + 15.0, combo.center().y), egui::Align2::LEFT_CENTER, name, crate::theme::small(), t.text);
    }
    icons::paint(p, Rect::from_center_size(pos2(combo.right() - 10.0, combo.center().y), vec2(10.0, 10.0)), Icon::ChevronDown, false);
    icons::describe_control(ui, &resp, "Current layer", None, false);
    egui::Popup::from_toggle_button_response(&resp).width(combo.width()).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for (name, on, frozen, locked, color) in &layers {
                ui.horizontal(|ui| {
                    if icons::button(ui, if *on { Icon::Bulb } else { Icon::BulbOff }, 18.0, "On/Off", false).clicked() {
                        clicks.layer_toggle = Some((name.clone(), "on", !*on));
                    }
                    if icons::button(ui, if *frozen { Icon::Snowflake } else { Icon::Sun }, 18.0, "Freeze/Thaw", false).clicked() {
                        clicks.layer_toggle = Some((name.clone(), "frozen", !*frozen));
                    }
                    if icons::button(ui, if *locked { Icon::Lock } else { Icon::Unlock }, 18.0, "Lock/Unlock", false).clicked() {
                        clicks.layer_toggle = Some((name.clone(), "locked", !*locked));
                    }
                    let (sr, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                    crate::palettes::swatch(ui.painter(), sr, *color);
                    if ui.selectable_label(name.eq_ignore_ascii_case(&cur), name).clicked() {
                        clicks.layer_current = Some(name.clone());
                        ui.close();
                    }
                });
            }
        });
    });
    // Make Current / Match Layer.
    let row = Rect::from_min_size(pos2(rect.left(), combo.bottom() + 1.0), vec2(rect.width(), ROW_H));
    let half = row.width() / 2.0;
    for (i, (icon, cmd, label)) in
        [(Icon::MakeCurrent, "laymcur", "Make Current"), (Icon::LayerMatch, "laymch", "Match Layer")].into_iter().enumerate()
    {
        let br = Rect::from_min_size(pos2(row.left() + i as f32 * half, row.top()), vec2(half, ROW_H));
        let resp = ui.interact(br, ui.id().with(("rb_lay2", cmd)), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(br, 3.0, t.control_hover.gamma_multiply(0.6));
        }
        icons::paint(ui.painter(), Rect::from_min_size(pos2(br.left() + 2.0, br.center().y - 9.0), vec2(18.0, 18.0)), icon, false);
        ui.painter().text(pos2(br.left() + 24.0, br.center().y), egui::Align2::LEFT_CENTER, crate::i18n::t(label), crate::theme::small(), t.text);
        icons::describe_control(ui, &resp, label, None, false);
        if resp.clicked() {
            clicks.cmd = Some(cmd.to_string());
        }
    }
}

/// Draw one panel (items and title) in `rect`.
fn draw_panel(app: &CadApp, ui: &mut egui::Ui, rect: Rect, panel: &Panel, fit: Fit, clicks: &mut Clicks) {
    let t = Tokens::get();
    let body = Rect::from_min_max(pos2(rect.left() + PAD, rect.top() + 3.0), pos2(rect.right() - PAD, rect.bottom() - PANEL_TITLE_H));
    let mut x = body.left();
    for item in panel.items {
        let w = item_w(ui, item, fit);
        let ir = Rect::from_min_size(pos2(x, body.top()), vec2(w, body.height()));
        match item {
            Item::Large(btn) => large_button(app, ui, ir, btn, clicks),
            Item::Smalls(btns) => {
                for (row, btn) in btns.iter().enumerate() {
                    let br = Rect::from_min_size(pos2(ir.left(), ir.top() + row as f32 * ROW_H), vec2(w, ROW_H - 1.0));
                    small_button(app, ui, br, btn, fit, clicks);
                }
            }
            Item::Layers => layer_controls(app, ui, ir, clicks),
        }
        x += w + 2.0;
    }
    // Title strip, with the slide-out arrow.
    let tr = Rect::from_min_max(pos2(rect.left() + 1.0, rect.bottom() - PANEL_TITLE_H), pos2(rect.right() - 1.0, rect.bottom() - 1.0));
    ui.painter().rect_filled(tr, 2.0, t.chrome_dark);
    let name = crate::i18n::t(panel.name);
    if panel.more.is_empty() {
        ui.painter().text(tr.center(), egui::Align2::CENTER_CENTER, name, crate::theme::small(), t.text_dim);
    } else {
        let resp = ui.interact(tr, ui.id().with(("rb_more", panel.name, rect.left() as i32)), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(tr, 2.0, t.control_hover.gamma_multiply(0.6));
        }
        let gw = label_w(ui, panel.name, crate::theme::small());
        let cx = tr.center().x - 7.0;
        ui.painter().text(pos2(cx, tr.center().y), egui::Align2::CENTER_CENTER, name, crate::theme::small(), t.text_dim);
        icons::paint(ui.painter(), Rect::from_center_size(pos2(cx + gw / 2.0 + 9.0, tr.center().y), vec2(8.0, 8.0)), Icon::ChevronDown, false);
        icons::describe_control(ui, &resp, &format!("More {} tools", panel.name), None, false);
        flyout_menu(&resp, "", panel.more, clicks);
    }
}

/// A panel collapsed to one button that opens the whole panel.
fn collapsed_panel(app: &CadApp, ui: &mut egui::Ui, rect: Rect, panel: &Panel, clicks: &mut Clicks) {
    let t = Tokens::get();
    let resp = ui.interact(rect, ui.id().with(("rb_col", panel.name)), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect.shrink(2.0), 3.0, t.control_hover.gamma_multiply(0.6));
    }
    let icon = panel.items.iter().find_map(|i| match i {
        Item::Large(btn) => Some(btn.icon),
        Item::Smalls(btns) => btns.first().map(|b| b.icon),
        Item::Layers => Some(Icon::Layers),
    });
    icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.center().x, rect.top() + 20.0), vec2(30.0, 30.0)), icon.unwrap_or(Icon::Menu), false);
    let (l1, l2) = large_lines(panel.name);
    ui.painter().text(pos2(rect.center().x, rect.top() + 38.0), egui::Align2::CENTER_TOP, &l1, crate::theme::small(), t.text);
    let mut y = rect.top() + 51.0;
    if let Some(l2) = l2 {
        ui.painter().text(pos2(rect.center().x, y), egui::Align2::CENTER_TOP, l2, crate::theme::small(), t.text);
        y += 13.0;
    }
    icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.center().x, y + 5.0), vec2(9.0, 9.0)), Icon::ChevronDown, false);
    icons::describe_control(ui, &resp, panel.name, None, false);
    egui::Popup::from_toggle_button_response(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        let w = panel_w(ui, panel, Fit::Full);
        let (pr, _) = ui.allocate_exact_size(vec2(w, BODY_H), Sense::hover());
        let before = clicks.cmd.is_some();
        draw_panel(app, ui, pr, panel, Fit::Full, clicks);
        if !before && clicks.cmd.is_some() {
            ui.close();
        }
    });
}

/// The title row: our application button, the Quick Access Toolbar, and the drawing's name.
/// `left` is where content may start (after the macOS traffic lights).
fn title_row(app: &mut CadApp, ui: &mut egui::Ui, row: Rect, left: f32, clicks: &mut Clicks) {
    let t = Tokens::get();
    // Application button: our own mark (never Autodesk's).
    let ab = Rect::from_min_size(pos2(left + 2.0, row.center().y - 12.0), vec2(30.0, 24.0));
    let aresp = ui.interact(ab, ui.id().with("rb_app"), Sense::click());
    ui.painter().rect_filled(ab, 4.0, if aresp.hovered() { t.accent } else { t.accent.gamma_multiply(0.85) });
    icons::paint(ui.painter(), ab.shrink2(vec2(8.0, 5.0)), Icon::Menu, false);
    icons::describe_control(ui, &aresp, "Application menu", None, false);
    // Recent drawings under the commands, as AutoCAD's application menu lists them.
    let recent: Vec<String> = app.ui.recent_files.iter().take(8).cloned().collect();
    let mut open_recent: Option<String> = None;
    egui::Popup::from_toggle_button_response(&aresp).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
        ui.set_min_width(230.0);
        if !recent.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::t("Open Recent")).small().color(t.text_dim));
            for path in &recent {
                let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
                if ui.add(egui::Button::new(name).frame(false)).on_hover_text(path).clicked() {
                    open_recent = Some(path.clone());
                }
            }
            ui.separator();
        }
        for (icon, cmd, label) in APP_MENU.iter().filter(|(_, c, _)| command_known(c)) {
            let (r, row) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
            if row.hovered() {
                ui.painter().rect_filled(r, 3.0, t.control_hover);
            }
            icons::paint(ui.painter(), Rect::from_min_size(pos2(r.left() + 4.0, r.top() + 4.0), vec2(20.0, 20.0)), *icon, false);
            ui.painter().text(pos2(r.left() + 32.0, r.center().y), egui::Align2::LEFT_CENTER, crate::i18n::t(label), crate::theme::body(), t.text);
            icons::describe_control(ui, &row, label, None, false);
            if row.clicked() {
                clicks.cmd = Some((*cmd).to_string());
            }
        }
    });
    if let Some(path) = open_recent {
        app.open_path(&path);
    }
    let mut x = ab.right() + 10.0;
    for (icon, cmd, tip) in QAT {
        let br = Rect::from_min_size(pos2(x, row.center().y - 11.0), vec2(22.0, 22.0));
        let resp = ui.interact(br, ui.id().with(("rb_qat", *cmd)), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(br, 3.0, t.control_hover.gamma_multiply(0.6));
        }
        icons::paint(ui.painter(), br.shrink(3.0), *icon, false);
        icons::describe_control(ui, &resp, tip, None, false);
        clicks.qat_rects.push(br);
        if resp.on_hover_text(crate::i18n::t(tip)).clicked() {
            clicks.cmd = Some((*cmd).to_string());
        }
        x += 25.0;
    }
    let heart_left = support_button(ui, row, clicks);
    if !app.integrated_titlebar {
        let title = match app.session.state() {
            // `*`: unsaved changes, as on the drawing's tab.
            Ok(st) if !app.ui.start_tab => {
                format!(
                    "{}{}{}",
                    st.title,
                    if st.title.contains('.') { String::new() } else { format!(".{}", app.ui.save_format.as_str()) },
                    if st.is_dirty() { "*" } else { "" }
                )
            }
            _ => crate::i18n::t("Start").to_string(),
        };
        let g = ui.painter().layout_no_wrap(title, egui::FontId::proportional(13.0), t.text_dim);
        let tx = (row.center().x - g.size().x / 2.0).max(x + 16.0);
        if tx + g.size().x < heart_left - 12.0 {
            ui.painter().galley(pos2(tx, row.center().y - g.size().y / 2.0), g, t.text_dim);
        }
    }
}

/// The heart button at the right of the title row: Report a Bug, Request a Feature and (while
/// donations are on) Donate ([`crate::support`]). Returns its left edge.
fn support_button(ui: &mut egui::Ui, row: Rect, clicks: &mut Clicks) -> f32 {
    let t = Tokens::get();
    let label = crate::i18n::t(crate::support::button_label());
    let g = ui.painter().layout_no_wrap(label.to_string(), crate::theme::body(), t.text);
    let w = g.size().x + 36.0;
    let br = Rect::from_min_size(pos2(row.right() - w - 8.0, row.center().y - 11.0), vec2(w, 22.0));
    let resp = ui.interact(br, ui.id().with("rb_support"), Sense::click());
    let blue = egui::Color32::from_rgb(0x1f, 0x6b, 0xff);
    ui.painter().rect(
        br,
        11.0,
        if resp.hovered() { blue.gamma_multiply(0.22) } else { blue.gamma_multiply(0.10) },
        Stroke::new(1.0, blue.gamma_multiply(0.7)),
        egui::StrokeKind::Inside,
    );
    crate::support::paint_heart(ui.painter(), pos2(br.left() + 15.0, br.center().y), 12.0, blue);
    ui.painter().galley(pos2(br.left() + 26.0, br.center().y - g.size().y / 2.0), g, t.text);
    icons::describe_control(ui, &resp, crate::support::button_label(), None, false);
    egui::Popup::from_toggle_button_response(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
        ui.set_min_width(230.0);
        for (cmd, label) in crate::support::entries() {
            if ui.add(egui::Button::new(crate::i18n::t(label)).frame(false).min_size(vec2(ui.available_width(), 26.0))).clicked() {
                clicks.cmd = Some(cmd.to_string());
            }
        }
        ui.separator();
        ui.label(egui::RichText::new(crate::tf!("Or email {address}", address = crate::support::FEEDBACK_EMAIL)).small().color(t.text_dim));
    });
    br.left()
}

/// The tabs row and the active tab's panels.
fn tabs_and_body(app: &mut CadApp, ui: &mut egui::Ui, area: Rect, clicks: &mut Clicks) {
    let t = Tokens::get();
    let tabs_r = Rect::from_min_size(area.min, vec2(area.width(), TAB_H));
    let mut x = tabs_r.left() + 8.0;
    // The current workspace's tabs.
    let shown: Vec<&Tab> = TABS.iter().filter(|tb| crate::workspaces::tab_shown(app, tb.name)).collect();
    let Some(active) = shown.iter().copied().find(|tb| tb.name == app.ui.ribbon_tab).or(shown.first().copied()) else { return };
    for tab in shown {
        let w = label_w(ui, tab.name, crate::theme::body()) + 22.0;
        let tr = Rect::from_min_size(pos2(x, tabs_r.top()), vec2(w, TAB_H));
        let on = std::ptr::eq(tab, active);
        let resp = ui.interact(tr, ui.id().with(("rb_tab", tab.name)), Sense::click());
        if on {
            ui.painter().rect_filled(tr, egui::CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, t.panel);
        } else if resp.hovered() {
            ui.painter().rect_filled(tr, 3.0, t.control_hover.gamma_multiply(0.4));
        }
        ui.painter().text(
            tr.center(),
            egui::Align2::CENTER_CENTER,
            crate::i18n::t(tab.name),
            crate::theme::body(),
            if on { t.accent } else { t.text },
        );
        icons::describe_control(ui, &resp, tab.name, Some(on), true);
        clicks.tab_rects.push((tr, tab.name));
        if resp.clicked() {
            clicks.tab = Some(tab.name.to_string());
        }
        x += w + 2.0;
    }
    // Body.
    let body = Rect::from_min_max(pos2(area.left(), tabs_r.bottom()), area.max);
    ui.painter().rect_filled(body, 0.0, t.panel);
    ui.painter().hline(body.x_range(), body.bottom() - 0.5, Stroke::new(1.0, t.border));
    let avail = body.width() - 8.0;
    let mut fit = Fit::Full;
    let mut widths: Vec<f32> = active.panels.iter().map(|p| panel_w(ui, p, fit)).collect();
    if widths.iter().sum::<f32>() > avail {
        fit = Fit::Compact;
        widths = active.panels.iter().map(|p| panel_w(ui, p, fit)).collect();
    }
    // Collapse from the right until the panels fit.
    let mut collapsed = vec![false; widths.len()];
    let total = |w: &[f32], c: &[bool]| w.iter().zip(c).map(|(w, c)| if *c { COLLAPSED_W } else { *w }).sum::<f32>();
    for i in (0..widths.len()).rev() {
        if total(&widths, &collapsed) <= avail {
            break;
        }
        if let Some(c) = collapsed.get_mut(i) {
            *c = true;
        }
    }
    let mut x = body.left() + 4.0;
    for ((panel, w), c) in active.panels.iter().zip(&widths).zip(&collapsed) {
        let pw = if *c { COLLAPSED_W } else { *w };
        if x + pw > body.right() {
            break;
        }
        let pr = Rect::from_min_size(pos2(x, body.top() + 2.0), vec2(pw, body.height() - 4.0));
        if *c {
            collapsed_panel(app, ui, pr, panel, clicks);
        } else {
            draw_panel(app, ui, pr, panel, fit, clicks);
        }
        x += pw;
        ui.painter().vline(x, (pr.top() + 4.0)..=(pr.bottom() - 4.0), Stroke::new(1.0, t.border));
        x += 1.0;
    }
}

/// The whole top of the window in ribbon mode: title row, tabs and the active tab's panels.
/// `title_h` is the macOS integrated title-bar height (0 elsewhere).
pub fn show(app: &mut CadApp, ui: &mut egui::Ui, title_h: f32, content_left: f32) {
    let t = Tokens::get();
    let row_h = TITLE_H.max(title_h);
    egui::Panel::top("cc_ribbon").exact_size(row_h + height()).frame(egui::Frame::NONE.fill(t.chrome)).show(ui, |ui| {
        let r = ui.max_rect();
        let mut clicks = Clicks::default();
        let row = Rect::from_min_size(r.min, vec2(r.width(), row_h));
        if title_h > 0.0 {
            // Window dragging on the title row, as the tool bar does.
            let resp = ui.interact(row, ui.id().with("rb_titledrag"), Sense::CLICK | Sense::DRAG);
            if resp.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if resp.double_clicked() {
                let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            }
        }
        title_row(app, ui, row, r.left() + content_left, &mut clicks);
        tabs_and_body(app, ui, Rect::from_min_max(pos2(r.left(), row.bottom()), r.max), &mut clicks);
        // Key tips (Alt): letters over the tabs and Quick Access buttons, then over the buttons.
        let names: Vec<&str> = TABS.iter().map(|t| t.name).collect();
        let qat: Vec<&str> = QAT.iter().map(|(_, c, _)| *c).collect();
        let buttons: Vec<(&str, &str)> = clicks.buttons.iter().map(|(_, l, c)| (*l, *c)).collect();
        let asked = crate::keytips::input(ui.ctx(), &names, &qat, &buttons);
        match crate::keytips::level(ui.ctx()) {
            crate::keytips::Level::Tabs => {
                for (r, name) in &clicks.tab_rects {
                    crate::keytips::badge(ui.painter(), *r, &crate::keytips::tab_tip(name));
                }
                for (i, r) in clicks.qat_rects.iter().enumerate().take(9) {
                    crate::keytips::badge(ui.painter(), *r, &(i + 1).to_string());
                }
            }
            crate::keytips::Level::Buttons => {
                let labels: Vec<&str> = clicks.buttons.iter().map(|(_, l, _)| *l).collect();
                for ((r, _, _), tip) in clicks.buttons.iter().zip(crate::keytips::assign(&labels)) {
                    crate::keytips::badge(ui.painter(), *r, &tip);
                }
            }
            crate::keytips::Level::Off => {}
        }
        if asked.tab.is_some() {
            clicks.tab = asked.tab;
        }
        if asked.cmd.is_some() {
            clicks.cmd = asked.cmd;
        }
        if let Some(tab) = clicks.tab {
            app.ui.ribbon_tab = tab;
        }
        if let Some((key, cmd)) = clicks.recent {
            app.ui.ribbon_recent.retain(|(k, _)| *k != key);
            app.ui.ribbon_recent.push((key, cmd));
        }
        if let Some(n) = clicks.layer_current {
            let _ = app.run("layer.current", json!({ "name": n }));
        }
        if let Some((n, k, v)) = clicks.layer_toggle {
            crate::layers::set_flag(app, &n, k, v);
        }
        if let Some(cmd) = clicks.cmd {
            app.start(&cmd);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ribbon command exists, so no button is dead.
    #[test]
    fn every_ribbon_command_exists() {
        let mut missing = Vec::new();
        let mut check = |c: &str| {
            if !command_known(c) {
                missing.push(c.to_string());
            }
        };
        for tab in TABS {
            for panel in tab.panels {
                for item in panel.items {
                    let btns: Vec<&Btn> = match item {
                        Item::Large(b) => vec![b],
                        Item::Smalls(bs) => bs.iter().collect(),
                        Item::Layers => vec![],
                    };
                    for btn in btns {
                        check(btn.cmd);
                        btn.flyout.iter().for_each(|(_, c, _)| check(c));
                    }
                }
                panel.more.iter().for_each(|(_, c, _)| check(c));
            }
        }
        QAT.iter().chain(APP_MENU).for_each(|(_, c, _)| check(c));
        crate::support::entries().iter().for_each(|(c, _)| check(c));
        assert!(missing.is_empty(), "ribbon commands that don't exist: {missing:?}");
    }

    /// AutoCAD's tab order, so the tabs are where an AutoCAD user looks.
    #[test]
    fn tabs_follow_autocad_order() {
        let names: Vec<_> = TABS.iter().map(|t| t.name).collect();
        assert_eq!(names, ["Home", "Insert", "Annotate", "Parametric", "View", "Manage", "Output"]);
        let home: Vec<_> = TABS[0].panels.iter().map(|p| p.name).collect();
        assert_eq!(home, ["Draw", "Modify", "Annotation", "Layers", "Block", "Properties", "Groups", "Utilities", "Clipboard"]);
    }
}
