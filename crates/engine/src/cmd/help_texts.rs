//! Hand-written help text for the most-used commands.
//!
//! Each entry maps a command id (lowercase) to a help string shown in the help browser.
//! The text is plain prose (not Markdown); the help browser adds the structured header
//! (aliases, menu path, shortcut, params) from the [`CommandSpec`].

/// Return the hand-written help text for `id`, or `""` if none exists.
pub fn lookup(id: &str) -> &'static str {
    match id {
        // ── File ──────────────────────────────────────────────────────────────────
        "new" => {
            "Creates a new drawing. The new drawing starts with the default template \
                  (units, layers, styles) or a template you choose. Use NEW to begin a fresh \
                  design without closing the current one."
        }
        "open" => {
            "Opens an existing drawing file. Supports DWG, DXF and CDY formats. \
                   The file appears in a new drawing tab. Read-only files open but cannot be saved \
                   back to the same path."
        }
        "close" => {
            "Closes the current drawing. If the drawing has unsaved changes you are asked \
                    whether to save, discard or cancel. CLOSE removes the drawing tab; the \
                    application stays open."
        }
        "closeall" => "Closes every open drawing, asking about unsaved changes for each one.",
        "qsave" => {
            "Saves the current drawing to its file. If the drawing has never been saved, \
                    QSAVE behaves like SAVEAS and asks for a file name and format."
        }
        "saveas" => {
            "Saves the current drawing under a new name or in a different format \
                     (DWG, DXF or CDY). The drawing tab updates to the new name."
        }
        "recover" => {
            "Opens a damaged drawing and attempts to repair it. Objects that cannot be \
                      read are reported on the command line."
        }
        "audit" => {
            "Checks the current drawing for errors and optionally fixes them. Reports the \
                    number of objects checked and errors found or repaired."
        }
        "export" => {
            "Exports the drawing or selection to another format (PDF, DWG, DXF, IFC, 3MF, \
                     STL and others)."
        }
        "import" => "Imports geometry from another file into the current drawing.",
        "plot" => {
            "Prints or exports the current layout to a printer or PDF. Opens the Plot dialog \
                   where you choose the device, paper size, area, scale and plot style table."
        }
        "-plot" => {
            "Command-line plot: asks each setting as a prompt (device, paper, units, \
                    orientation, area, scale, offset, plot style table, lineweights, shade mode, \
                    file name, save changes, proceed)."
        }
        "publish" => {
            "Publishes multiple sheets to PDF, DWF or a printer. Use a sheet set or pick \
                      layouts from open drawings."
        }
        "etransmit" => {
            "Packages the drawing and all its dependent files (xrefs, images, fonts, \
                        plot styles) into a folder or ZIP for sending."
        }
        "dwgconvert" => {
            "Converts drawings in batch between CDY, DWG and DXF. DWG and DXF files can be \
             written in an older version (2000 to 2018, or Release 12 DXF)."
        }
        "convert" => "Opens the Convert Drawings dialog for batch file format conversion.",

        // ── Drawing ───────────────────────────────────────────────────────────────
        "line" => {
            "Draws straight line segments. Pick points to define each segment; press Enter \
                   or right-click to finish. Use Close to connect the last point back to the first."
        }
        "circle" => {
            "Draws a circle. The default method is center and radius; options include \
                     center and diameter, two points, three points, and tangent-tangent-radius."
        }
        "arc" => {
            "Draws a circular arc. The default method is three points; options include \
                  start/center/end, start/center/angle, start/end/angle, start/end/radius and \
                  continuation from the last line or arc."
        }
        "pline" => {
            "Draws a polyline: connected line and arc segments that form a single object. \
                    Options include setting the width, drawing arcs, undoing the last segment, \
                    and closing the polyline."
        }
        "rectang" => {
            "Draws a rectangle as a closed polyline. Pick two opposite corners; options \
                      include chamfer, fillet, width and elevation."
        }
        "polygon" => {
            "Draws a regular polygon (3 to 1024 sides) as a closed polyline, inscribed \
                      in or circumscribed about a circle."
        }
        "ellipse" => {
            "Draws an ellipse or elliptical arc. Specify the two endpoints of one axis \
                      and a distance for the other, or use the Center option."
        }
        "spline" => "Draws a smooth curve (NURBS spline) through fit points or by control vertices.",
        "xline" => "Draws an infinite construction line through two points.",
        "ray" => "Draws a semi-infinite construction line from a start point through a second point.",
        "mline" => "Draws a multiline: parallel lines with optional caps and fills.",
        "point" => "Places a point object. The point style (PDMODE) controls how points appear.",
        "divide" => "Places point objects or blocks at equal intervals along an object.",
        "measure" => "Places point objects or blocks at measured intervals along an object.",
        "region" => {
            "Converts closed shapes (polylines, circles, ellipses, splines) into region \
                     objects for Boolean operations."
        }
        "boundary" => "Creates a polyline or region from an enclosed area.",
        "revcloud" => "Draws a revision cloud to mark areas that have changed.",
        "donut" => "Draws a filled ring (donut) with an inside and outside diameter.",
        "helix" => "Draws a 3D helix (spiral) with a base radius, top radius, height and turns.",
        "mtext" => {
            "Creates multiline text with formatting (bold, italic, fonts, stacking, \
                    columns). The in-place editor opens when you define the text boundary."
        }
        "text" => {
            "Creates single-line text. Pick an insertion point, set the height and rotation, \
                   then type. Press Enter twice to finish."
        }
        "table" => {
            "Inserts a table with rows and columns. Specify the number of columns and rows \
                    or pick a size on screen."
        }
        "hatch" => {
            "Fills an enclosed area with a pattern or solid colour. Pick an internal point \
                    or select boundary objects; choose the pattern, scale and angle."
        }
        "gradient" => "Fills an enclosed area with a colour gradient.",

        // ── Editing ───────────────────────────────────────────────────────────────
        "erase" => "Deletes selected objects from the drawing.",
        "copy" => {
            "Duplicates selected objects at a displacement you specify. Pick a base point \
                   and one or more destination points; press Enter to finish."
        }
        "move" => "Moves selected objects. Pick a base point and a destination point.",
        "rotate" => {
            "Rotates selected objects about a base point by an angle you specify. \
                     The Reference option lets you align to a known angle."
        }
        "scale" => {
            "Resizes selected objects uniformly about a base point by a scale factor. \
                    The Reference option lets you scale to a known length."
        }
        "mirror" => "Creates a mirrored copy of selected objects across a line you pick.",
        "offset" => {
            "Creates a parallel copy of a line, arc, circle or polyline at a specified \
                     distance or through a point."
        }
        "trim" => {
            "Trims objects to a cutting edge. Select cutting edges first, then pick the \
                   parts to remove. In quick mode (the default) all objects act as edges."
        }
        "extend" => {
            "Extends objects to meet a boundary edge. Select boundary edges first, then \
                     pick the objects to extend."
        }
        "fillet" => {
            "Rounds the corner between two lines, arcs or polyline segments with an arc \
                     of a specified radius. A radius of 0 produces a sharp corner."
        }
        "chamfer" => {
            "Bevels the corner between two lines with a straight line. Specify two \
                      distances or a distance and an angle."
        }
        "stretch" => {
            "Stretches objects that cross a selection window. Objects fully inside the \
                      window move; objects crossing the window boundary stretch."
        }
        "array" => "Creates copies of objects in a rectangular, polar or path pattern.",
        "arrayrect" => "Creates a rectangular array with rows, columns and optional levels.",
        "arraypolar" => "Creates a polar (circular) array about a centre point.",
        "arraypath" => "Creates copies along a path (line, arc, polyline or spline).",
        "break" => "Splits an object at one or two points, removing the segment between them.",
        "join" => "Combines collinear lines, coplanar arcs or polyline segments into one object.",
        "explode" => {
            "Breaks a compound object (block, polyline, hatch, dimension, mline) into \
                      its individual parts."
        }
        "pedit" => {
            "Edits polylines and 3D meshes. Options include Open, Close, Join nearby \
                    segments, change Width, Fit curve, Spline, Decurve, and edit individual \
                    vertices."
        }
        "matchprop" => {
            "Copies properties (layer, color, linetype, lineweight, plot style and \
                        others) from a source object to destination objects."
        }
        "chprop" => {
            "Changes properties of selected objects: color, layer, linetype, lineweight, \
                     transparency, linetype scale, thickness and material."
        }
        "undo" => {
            "Reverses the most recent operation. Repeat to step back further. Mark and Back \
                   let you undo to a saved point."
        }
        "redo" => "Reverses the last UNDO. Only available immediately after an UNDO.",
        "oops" => "Restores the most recently erased objects, even after other commands.",
        "draworder" => {
            "Changes the display order of overlapping objects (bring to front, send to \
                        back, above/below a reference object)."
        }
        "align" => {
            "Moves, rotates and scales objects in one step by mapping source points to \
                    destination points (1, 2 or 3 pairs)."
        }
        "lengthen" => {
            "Changes the length of an arc or open curve by a delta, a percentage, a \
                       total or dynamically."
        }

        // ── Blocks ────────────────────────────────────────────────────────────────
        "block" => {
            "Defines a block from selected objects. Give it a name, pick a base point, and \
                    choose whether to keep, convert or delete the originals."
        }
        "-block" => "Command-line version of BLOCK (no dialog).",
        "insert" => {
            "Inserts a block reference into the drawing. Specify the name, insertion \
                     point, scale and rotation."
        }
        "wblock" => "Writes a block or selected objects to a new drawing file.",
        "bedit" => "Opens the Block Editor so you can modify a block definition in place.",
        "bclose" => "Closes the Block Editor and saves or discards changes.",
        "refedit" => "Edits a block or xref in place without opening the Block Editor.",
        "refclose" => "Closes an in-place reference edit and saves or discards changes.",
        "xref" => "Manages external references (xrefs): attach, detach, reload, unload, bind.",
        "xattach" => "Attaches an external drawing as an xref.",
        "xbind" => "Binds named items (layers, styles, blocks) from an xref into the drawing.",
        "purge" => {
            "Removes unused named items (blocks, layers, linetypes, text styles, dimension \
                    styles, materials and others) from the drawing."
        }
        "burst" => {
            "Explodes blocks and converts their attributes to text objects (unlike EXPLODE, \
                    which discards attribute values)."
        }

        // ── Layers ────────────────────────────────────────────────────────────────
        "layer" => {
            "Opens the Layer Properties Manager where you create, rename, delete and \
                    configure layers (color, linetype, lineweight, transparency, plot, freeze, \
                    lock, description)."
        }
        "laycur" => "Moves selected objects to the current layer.",
        "layiso" => "Isolates selected layers: turns off or fades all others.",
        "layuniso" => "Restores layers that were hidden by LAYISO.",
        "layoff" => "Turns off the layer of a selected object.",
        "layon" => "Turns on all layers.",
        "layfrz" => "Freezes the layer of a selected object.",
        "laythw" => "Thaws all layers.",
        "laylck" => "Locks the layer of a selected object so its objects cannot be edited.",
        "layulk" => "Unlocks the layer of a selected object.",
        "laydel" => "Deletes a layer and moves its objects to another layer.",
        "laymrg" => "Merges one layer into another and deletes the source layer.",
        "laymch" => "Moves objects to the layer of a chosen object.",

        // ── Views ─────────────────────────────────────────────────────────────────
        "zoom" => {
            "Changes the view magnification. Options include All, Center, Dynamic, Extents, \
                   Previous, Scale, Window, Object and Realtime."
        }
        "pan" => {
            "Shifts the view without changing the magnification. Drag to pan, or specify \
                  a displacement."
        }
        "regen" => "Regenerates the entire drawing, recalculating the display of every object.",
        "regenall" => "Regenerates all viewports.",
        "redraw" => "Refreshes the display of the current viewport.",
        "view" => "Saves and restores named views.",
        "3dorbit" => "Rotates the 3D view interactively. Drag to orbit around the target.",
        "3dwalk" => "Walks through a 3D model at eye level using keyboard and mouse controls.",
        "3dfly" => "Flies through a 3D model in any direction.",
        "vscurrent" => {
            "Sets the visual style of the current viewport (2D Wireframe, Conceptual, \
                        Realistic, Shaded, X-Ray and others)."
        }
        "visualstyles" => {
            "Sets how the Realistic visual style finishes its image: ambient occlusion in corners and \
                        creases, soft shadows from the sun's size, and the tone curve. Plots and exports \
                        of the drawing use the same settings."
        }
        "ucs" => "Defines or restores a user coordinate system.",

        // ── Dimensions ────────────────────────────────────────────────────────────
        "dimlinear" => "Creates a horizontal or vertical dimension.",
        "dimaligned" => "Creates a dimension aligned with the origin points.",
        "dimangular" => "Dimensions the angle between two lines, an arc, or three points.",
        "dimradius" => "Dimensions the radius of an arc or circle.",
        "dimdiameter" => "Dimensions the diameter of an arc or circle.",
        "dimordinate" => "Creates an ordinate dimension (the X or Y distance from an origin).",
        "dimbaseline" => "Creates a series of dimensions measured from the same baseline.",
        "dimcontinue" => "Creates a chain of dimensions, each starting where the last ended.",
        "dimstyle" => "Opens the Dimension Style Manager to create and modify dimension styles.",
        "-dimstyle" => {
            "Command-line dimension style management (Apply current overrides to \
                        selected dimensions, create, rename, delete styles)."
        }
        "qdim" => {
            "Creates a quick dimension from selected geometry: continuous, baseline, \
                   ordinate, staggered, radius or diameter."
        }
        "dim" => {
            "Starts the DIM command for continuous dimensioning with modes (linear, aligned, \
                  angular, radius, diameter, ordinate, baseline, continue, distribute)."
        }
        "qleader" => "Creates a leader with text annotation.",
        "mleader" => {
            "Creates a multileader: an arrow with a text or block label, formatted by an \
                      mleader style."
        }
        "mleaderstyle" => "Opens the Multileader Style Manager.",

        // ── Annotation ────────────────────────────────────────────────────────────
        "style" => {
            "Opens the Text Style dialog to create and modify text styles (font, height, \
                    width factor, oblique angle)."
        }
        "linetype" => "Opens the Linetype Manager to load and set linetypes.",
        "ltscale" => "Sets the global linetype scale factor.",
        "color" => "Sets the current colour for new objects.",

        // ── Inquiry ───────────────────────────────────────────────────────────────
        "dist" => "Measures the distance and angle between two points.",
        "area" => {
            "Calculates the area and perimeter of a region, polyline or sequence of points. \
                   Add and Subtract options let you combine multiple areas."
        }
        "list" => {
            "Displays detailed information about selected objects (type, layer, color, \
                   position, geometry)."
        }
        "id" => "Reports the coordinates of a picked point.",
        "measuregeom" => {
            "Measures distance, radius, angle or area interactively, with Quick mode \
                          for one-pick nearest-geometry measurements."
        }
        "massprop" => {
            "Calculates mass properties (area, perimeter, centroid, moments of inertia) \
                       for regions and solids."
        }

        // ── Selection ─────────────────────────────────────────────────────────────
        "select" => {
            "Builds a selection set using Window, Crossing, WPolygon, CPolygon, Fence, \
                     Last, Previous, All or a pick."
        }
        "selectall" => "Selects every object in the current space.",
        "qselect" => "Opens Quick Select to select objects by property criteria.",
        "selectsimilar" => {
            "Selects all objects that share properties with the selected object \
                            (layer, color, linetype, name, style or block name)."
        }
        "filter" => "Creates or applies a named selection filter.",

        // ── Settings ──────────────────────────────────────────────────────────────
        "osnap" => {
            "Sets running object snap modes (endpoint, midpoint, center, quadrant, \
                    intersection, extension, perpendicular, tangent, nearest, node, insert, \
                    none)."
        }
        "snap" => "Constrains the cursor to a grid of snap points at a specified spacing.",
        "grid" => "Displays a grid of dots or lines at a specified spacing as a visual reference.",
        "ortho" => "Constrains cursor movement to horizontal and vertical directions.",
        "limits" => "Sets the drawing limits (the area covered by the grid).",
        "units" => "Sets the format and precision of coordinates and angles.",
        "options" => {
            "Opens the Options dialog for application-wide settings (display, files, \
                      system, user preferences, drafting, 3D, paths)."
        }

        // ── 3D Solids ─────────────────────────────────────────────────────────────
        "box" => "Creates a 3D solid box.",
        "sphere" => "Creates a 3D solid sphere.",
        "cylinder" => "Creates a 3D solid cylinder.",
        "cone" => "Creates a 3D solid cone.",
        "torus" => "Creates a 3D solid torus (ring).",
        "wedge" => "Creates a 3D solid wedge.",
        "pyramid" => "Creates a 3D solid pyramid.",
        "extrude" => {
            "Creates a 3D solid or surface by extruding a 2D profile along a path or \
                      by a height."
        }
        "revolve" => "Creates a 3D solid or surface by revolving a 2D profile about an axis.",
        "loft" => "Creates a 3D solid or surface through a series of cross-section profiles.",
        "sweep" => "Creates a 3D solid or surface by sweeping a profile along a path.",
        "union" => "Combines two or more solids or regions into one.",
        "subtract" => "Subtracts one set of solids or regions from another.",
        "intersect" => "Creates a solid or region from the common volume of two or more.",
        "thicken" => "Converts a surface into a solid by thickening it.",
        "3dmove" => "Moves objects in 3D along an axis, a plane or freely.",
        "3drotate" => "Rotates objects in 3D about an axis.",

        // ── Layouts & Viewports ───────────────────────────────────────────────────
        "layout" => "Creates, renames, copies or deletes layouts (paper-space tabs).",
        "mspace" => "Switches to model space inside a paper-space viewport.",
        "pspace" => "Switches to paper space from a model-space viewport.",
        "mview" => "Creates and controls layout viewports.",
        "pagesetup" => {
            "Opens the Page Setup dialog for the current layout (printer, paper size, \
                        plot area, scale, plot style table)."
        }
        "vpclip" => "Clips a layout viewport to a polyline, circle or other closed shape.",

        // ── Xrefs & Images ───────────────────────────────────────────────────────
        "imageattach" => "Attaches a raster image to the drawing.",
        "imageclip" => "Clips an attached image to a rectangular or polygonal boundary.",
        "pdfattach" => "Attaches a PDF file as an underlay.",
        "dwfattach" => "Attaches a DWF/DWFx file as an underlay.",

        // ── Data & Attributes ─────────────────────────────────────────────────────
        "attdef" => "Creates an attribute definition for use in a block.",
        "attedit" => "Edits attribute values in a block reference.",
        "-attedit" => "Command-line attribute editing.",
        "battman" => "Opens the Block Attribute Manager to edit attribute definitions in blocks.",
        "dataextraction" => "Extracts object and attribute data to a table or external file.",
        "field" => {
            "Inserts a field that updates automatically (date, file name, area, sheet \
                    number and others)."
        }

        // ── Express Tools ─────────────────────────────────────────────────────────
        "overkill" => "Removes duplicate and overlapping geometry.",
        "flatten" => "Projects 3D objects onto the XY plane, creating a 2D representation.",
        "textfit" => "Stretches or compresses single-line text to fit between two points.",
        "torient" => "Reorients text objects to be readable (not upside-down or mirrored).",
        "scaletext" => "Scales text objects about their insertion points without moving them.",
        "justifytext" => "Changes the justification of text objects without moving them on screen.",

        // ── BIM & Walk ────────────────────────────────────────────────────────────
        "wallgen" => "Generates 3D walls, floors and ceilings from a 2D floor plan.",
        "lightbake" => "Bakes indirect lighting into the 3D model for realistic walkthroughs.",

        // ── Security ──────────────────────────────────────────────────────────────
        "appload" => "Loads an AutoLISP (.lsp) application into the current session.",

        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::cmd::{command_specs, commands_without_help, find_by_id_or_alias, find_command};
    use crate::{EngineError, Input, Session};

    fn page(id: &str) -> crate::cmd::HelpPage {
        find_command(id).map(|c| c.help_page()).unwrap_or_else(|| panic!("no command {id}"))
    }

    /// The keys of [`lookup`], read from this file's source: every `"key" =>` arm before the tests.
    fn text_keys() -> Vec<&'static str> {
        let src = include_str!("help_texts.rs");
        let body = src.split("#[cfg(test)]").next().unwrap_or("");
        body.lines().filter_map(|l| l.trim_start().strip_prefix('"')?.split_once("\" =>").map(|(k, _)| k)).collect()
    }

    #[test]
    fn zoom_page_lists_its_options_and_prompt() {
        let p = page("zoom");
        assert_eq!(p.options, ["All", "Center", "Dynamic", "Extents", "Previous", "Scale", "Window", "Object"]);
        assert_eq!(
            p.prompts,
            ["Specify corner of window, enter a scale factor (nX or nXP) or [All/Center/Dynamic/Extents/Previous/Scale/Window/Object] <real time>:"]
        );
    }

    #[test]
    fn circle_page_lists_its_options_and_prompt() {
        let p = page("circle");
        assert_eq!(p.options, ["3P", "2P", "Ttr (tan tan radius)"]);
        assert_eq!(p.prompts, ["Specify center point for circle or [3P/2P/Ttr (tan tan radius)]:"]);
        // The option is typed TTR: the running command takes it as that keyword.
        let mut s = Session::new();
        s.start("circle").unwrap();
        assert_eq!(s.current_prompt().and_then(|q| q.match_keyword("TTR")).as_deref(), Some("Ttr (tan tan radius)"));
    }

    /// A command without prompts, or one that can't start on an empty drawing, has no options.
    #[test]
    fn pages_without_prompts_have_no_options() {
        let p = page("regen");
        assert!(p.options.is_empty() && p.prompts.is_empty(), "{p:?}");
    }

    /// What the page offers is what the command asks when started at the command line: every
    /// interactive command whose first prompt (in a session, through `Session::start`) has
    /// keywords has exactly those as the page's options.
    #[test]
    fn every_page_offers_the_keywords_of_the_first_prompt() {
        let mut checked = 0;
        let mut wrong = Vec::new();
        for spec in command_specs().iter().filter(|c| c.interactive.is_some()) {
            let mut s = Session::new();
            if s.start(spec.id).is_err() || s.running.as_ref().is_none_or(|r| r.id != spec.id) {
                continue;
            }
            let Some(asked) = s.running.as_ref().map(|r| r.machine.prompt(&s)) else { continue };
            if asked.keywords.is_empty() {
                continue;
            }
            checked += 1;
            let p = spec.help_page();
            if p.options != asked.keywords {
                wrong.push(format!("{}: page {:?}, prompt {:?}", spec.id, p.options, asked.keywords));
            }
        }
        assert!(wrong.is_empty(), "{} of {checked} differ:\n{}", wrong.len(), wrong.join("\n"));
        assert!(checked > 100, "only {checked} commands with keywords checked");
    }

    /// Known truth from the commands' registrations (not from the page code).
    #[test]
    fn pages_show_known_aliases_menus_and_params() {
        let line = page("line");
        assert_eq!(line.id, "LINE");
        assert_eq!(line.aliases, ["L"]);
        assert_eq!(line.menu_path, "Draw > Line");
        assert_eq!(line.params, "{points: [[x,y],...], closed?: bool}");
        assert!(line.interactive && !line.transparent);

        let circle = page("circle");
        assert_eq!(circle.aliases, ["C"]);
        assert_eq!(circle.menu_path, "Draw > Circle > Center, Radius");
        assert_eq!(circle.params, "{center, radius} | {center, diameter} | {p1, p2} | {p1, p2, p3}");

        let erase = page("erase");
        assert_eq!(erase.aliases, ["E", "DELETE"]);
        assert_eq!(erase.menu_path, "Modify > Erase");
        assert_eq!(erase.params, "{handles?: [hex]} (default: selection)");

        let zoom = page("zoom");
        assert_eq!(zoom.aliases, ["Z"]);
        assert_eq!(zoom.menu_path, "");
        assert!(zoom.transparent);
    }

    /// Every hand-written text is keyed by a real command id (not an alias or a typo), so it shows.
    #[test]
    fn every_text_key_is_a_command_id() {
        let keys = text_keys();
        assert!(keys.len() > 150, "read only {} keys", keys.len());
        let dead: Vec<_> = keys.iter().filter(|k| find_command(k).is_none_or(|c| c.id != **k)).collect();
        assert!(dead.is_empty(), "help texts keyed to no command: {dead:?}");
        // The arms reach the registry: each key's command carries its text.
        for k in keys {
            assert_eq!(find_command(k).map(|c| c.help), Some(lookup(k)), "{k}");
        }
        assert!(!page("layfrz").help_text.is_empty(), "LAYFRZ has its text");
    }

    #[test]
    fn lookup_returns_text_for_known_commands() {
        assert!(lookup("line").starts_with("Draws"), "{}", lookup("line"));
        assert!(!lookup("circle").is_empty());
        assert!(lookup("nonexistent_command_xyz").is_empty());
    }

    #[test]
    fn find_by_alias() {
        assert_eq!(find_by_id_or_alias("c").map(|c| c.id), Some("circle"));
        assert_eq!(find_by_id_or_alias("L").map(|c| c.id), Some("line"));
        assert_eq!(find_by_id_or_alias("cdyconvert").map(|c| c.id), Some("dwgconvert"));
        assert!(find_by_id_or_alias("nosuch").is_none());
    }

    #[test]
    fn help_command_gives_a_page_by_alias() {
        let mut s = Session::new();
        let v = s.execute("help", &json!({"command": "c"})).unwrap();
        assert_eq!(v["id"], "CIRCLE");
        assert_eq!(v["aliases"], json!(["C"]));
        assert_eq!(v["options"], json!(["3P", "2P", "Ttr (tan tan radius)"]));
        assert_eq!(v["prompts"][0], "Specify center point for circle or [3P/2P/Ttr (tan tan radius)]:");
        assert_eq!(v["menuPath"], "Draw > Circle > Center, Radius");
        // `?` is HELP typed short.
        assert_eq!(find_by_id_or_alias("?").map(|c| c.id), Some("help"));
        let v = s.execute("help", &json!({"command": "l"})).unwrap();
        assert_eq!(v["id"], "LINE");
    }

    #[test]
    fn help_command_refuses_an_unknown_name() {
        let mut s = Session::new();
        match s.execute("help", &json!({"command": "nosuch"})) {
            Err(EngineError::UnknownCommand(n)) => assert_eq!(n, "nosuch"),
            r => panic!("expected UnknownCommand, got {r:?}"),
        }
    }

    #[test]
    fn help_command_lists_in_pages() {
        let mut s = Session::new();
        let v = s.execute("help", &json!({})).unwrap();
        assert_eq!(v["count"], command_specs().len());
        assert_eq!(v["commands"].as_array().map(Vec::len), Some(100), "the default page");
        let v = s.execute("help", &json!({"filter": "circle", "limit": 5000})).unwrap();
        let ids: Vec<&str> = v["commands"].as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
        assert!(ids.contains(&"CIRCLE") && !ids.contains(&"LINE"), "{ids:?}");
        assert_eq!(v["count"], ids.len());
        let second = s.execute("help", &json!({"offset": 1, "limit": 1})).unwrap();
        let first_two = s.execute("help", &json!({"limit": 2})).unwrap();
        assert_eq!(second["commands"][0], first_two["commands"][1]);
    }

    /// The coverage report: HELP {missing: true} pages through `commands_without_help`.
    #[test]
    fn help_command_reports_commands_without_text() {
        let mut s = Session::new();
        let without = commands_without_help();
        let v = s.execute("help", &json!({"missing": true})).unwrap();
        assert_eq!(v["count"], without.len());
        assert_eq!(v["total"], command_specs().len());
        let mut listed = Vec::new();
        let mut offset = 0;
        loop {
            let v = s.execute("help", &json!({"missing": true, "offset": offset, "limit": 1000})).unwrap();
            let page: Vec<String> = v["missing"].as_array().unwrap().iter().filter_map(|c| c.as_str().map(str::to_string)).collect();
            if page.is_empty() {
                break;
            }
            offset += page.len();
            listed.extend(page);
        }
        let expected: Vec<String> = without.iter().map(|id| id.to_ascii_uppercase()).collect();
        assert_eq!(listed, expected);
        // LINE has hand-written text, so it is not listed.
        assert!(!listed.iter().any(|id| id == "LINE"));
        assert!(!find_command("line").is_none_or(|c| c.help.is_empty()));
        let first = without.first().map(|id| id.to_ascii_uppercase());
        assert_eq!(listed.first(), first.as_ref());
        // At least 100 commands carry text.
        assert!(command_specs().len() - without.len() >= 100);
    }

    /// A HELP typed at the command line answers, and does not end the running command.
    #[test]
    fn transparent_help_keeps_the_command() {
        let mut s = Session::new();
        s.start("line").unwrap();
        s.input(Input::Text("0,0".into())).unwrap();
        s.start("'help").unwrap();
        assert_eq!(s.running.as_ref().map(|r| r.id.as_str()), Some("line"));
    }

    /// Every command has help text: hand-written, or generated from its parameters (Q5279: an
    /// agent's HELP {command} came back with an empty helpText for every command it tried).
    #[test]
    fn every_command_has_help_text_and_an_example() {
        for spec in command_specs() {
            let page = spec.help_page();
            assert!(!page.help_text.trim().is_empty(), "{}", spec.id);
            assert_eq!(page.generated, spec.help.is_empty(), "{}", spec.id);
            assert!(page.example.starts_with(&format!("cadaddy-cli run DRAWING.dxf --cmd '{} {{", spec.id)), "{}", page.example);
            assert!(page.mcp_example.starts_with(&format!("execute {{command: \"{}\"", spec.id)), "{}", page.mcp_example);
            // Every documented top-level parameter is named in generated text.
            if page.generated {
                for p in crate::cmd::doc_params(crate::params_doc(spec)) {
                    assert!(page.help_text.contains(&p.name), "{}: {} not in {}", spec.id, p.name, page.help_text);
                }
            }
        }
    }

    #[test]
    fn help_command_returns_generated_text() {
        let mut s = Session::new();
        let v = s.execute("help", &json!({"command": "bim.takeoff"})).unwrap();
        let text = v["helpText"].as_str().unwrap();
        assert!(text.starts_with("Model Takeoff.") && text.contains("group_by (optional)"), "{text}");
        assert_eq!(v["generated"], json!(true));
        assert_eq!(v["example"], json!("cadaddy-cli run DRAWING.dxf --cmd 'bim.takeoff {}'"));
    }

    #[test]
    fn doc_params_reads_the_top_level_list() {
        use crate::cmd::{DocParam, doc_params};
        let p = |name: &str, optional: bool, kind: &str| DocParam { name: name.into(), optional, kind: kind.into() };
        assert_eq!(
            doc_params("{center: [x,y], radius?: number, opts?: {a, b: 1}, mode: \"a, b\" | c} → {handle}"),
            vec![p("center", false, "[x,y]"), p("radius", true, "number"), p("opts", true, "{a, b: 1}"), p("mode", false, "\"a, b\" | c")]
        );
        assert_eq!(doc_params("{} → {x}"), vec![]);
        assert_eq!(doc_params(""), vec![]);
        assert_eq!(doc_params("{id, all? (default false)}"), vec![p("id", false, ""), p("all", true, "(default false)")]);
        // Hostile docs never panic.
        for d in ["{", "}", "{,,,}", "{\"", "{a:", "{?:}", "→{a}", "{é: 1, ü?}"] {
            let _ = doc_params(d);
        }
    }

    #[test]
    fn examples_use_documented_literals() {
        let ex = |id: &str| find_command(id).unwrap().example_params();
        assert_eq!(ex("help"), json!({"command": "LINE"}));
        assert_eq!(ex("job.start"), json!({"pdfs": "FILE"}));
        assert_eq!(ex("job.answer"), json!({"id": "TEXT", "answer": "TEXT"}));
    }
}
