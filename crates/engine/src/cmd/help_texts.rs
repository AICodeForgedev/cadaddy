//! Hand-written help text for the most-used commands.
//!
//! Each entry maps a command id (lowercase) to a help string shown in the help browser.
//! The text is plain prose (not Markdown); the help browser adds the structured header
//! (aliases, menu path, shortcut, params) from the [`CommandSpec`].

/// Return the hand-written help text for `id`, or `""` if none exists.
pub fn lookup(id: &str) -> &'static str {
    match id {
        // ── File ──────────────────────────────────────────────────────────────────
        "new" => "Creates a new drawing. The new drawing starts with the default template \
                  (units, layers, styles) or a template you choose. Use NEW to begin a fresh \
                  design without closing the current one.",
        "open" => "Opens an existing drawing file. Supports DWG, DXF and CDY formats. \
                   The file appears in a new drawing tab. Read-only files open but cannot be saved \
                   back to the same path.",
        "close" => "Closes the current drawing. If the drawing has unsaved changes you are asked \
                    whether to save, discard or cancel. CLOSE removes the drawing tab; the \
                    application stays open.",
        "closeall" => "Closes every open drawing, asking about unsaved changes for each one.",
        "qsave" => "Saves the current drawing to its file. If the drawing has never been saved, \
                    QSAVE behaves like SAVEAS and asks for a file name and format.",
        "saveas" => "Saves the current drawing under a new name or in a different format \
                     (DWG, DXF or CDY). The drawing tab updates to the new name.",
        "save" => "Saves the current drawing. Equivalent to QSAVE for a named drawing; for an \
                   unnamed drawing, prompts for a file name.",
        "quit" => "Closes all drawings and exits the application. Unsaved drawings prompt for \
                   confirmation.",
        "qquit" => "Closes all drawings, asking about unsaved ones, then quits.",
        "recover" => "Opens a damaged drawing and attempts to repair it. Objects that cannot be \
                      read are reported on the command line.",
        "audit" => "Checks the current drawing for errors and optionally fixes them. Reports the \
                    number of objects checked and errors found or repaired.",
        "export" => "Exports the drawing or selection to another format (PDF, DWG, DXF, IFC, 3MF, \
                     STL and others).",
        "import" => "Imports geometry from another file into the current drawing.",
        "plot" => "Prints or exports the current layout to a printer or PDF. Opens the Plot dialog \
                   where you choose the device, paper size, area, scale and plot style table.",
        "-plot" => "Command-line plot: asks each setting as a prompt (device, paper, units, \
                    orientation, area, scale, offset, plot style table, lineweights, shade mode, \
                    file name, save changes, proceed).",
        "publish" => "Publishes multiple sheets to PDF, DWF or a printer. Use a sheet set or pick \
                      layouts from open drawings.",
        "etransmit" => "Packages the drawing and all its dependent files (xrefs, images, fonts, \
                        plot styles) into a folder or ZIP for sending.",
        "dwgconvert" => "Converts drawings between DWG versions (R12 through 2018) in batch.",
        "cdyconvert" => "Converts drawings between CDY and DWG/DXF formats.",
        "convert" => "Opens the Convert Drawings dialog for batch file format conversion.",

        // ── Drawing ───────────────────────────────────────────────────────────────
        "line" => "Draws straight line segments. Pick points to define each segment; press Enter \
                   or right-click to finish. Use Close to connect the last point back to the first.",
        "circle" => "Draws a circle. The default method is center and radius; options include \
                     center and diameter, two points, three points, and tangent-tangent-radius.",
        "arc" => "Draws a circular arc. The default method is three points; options include \
                  start/center/end, start/center/angle, start/end/angle, start/end/radius and \
                  continuation from the last line or arc.",
        "pline" => "Draws a polyline: connected line and arc segments that form a single object. \
                    Options include setting the width, drawing arcs, undoing the last segment, \
                    and closing the polyline.",
        "rectang" => "Draws a rectangle as a closed polyline. Pick two opposite corners; options \
                      include chamfer, fillet, width and elevation.",
        "polygon" => "Draws a regular polygon (3 to 1024 sides) as a closed polyline, inscribed \
                      in or circumscribed about a circle.",
        "ellipse" => "Draws an ellipse or elliptical arc. Specify the two endpoints of one axis \
                      and a distance for the other, or use the Center option.",
        "spline" => "Draws a smooth curve (NURBS spline) through fit points or by control vertices.",
        "xline" => "Draws an infinite construction line through two points.",
        "ray" => "Draws a semi-infinite construction line from a start point through a second point.",
        "mline" => "Draws a multiline: parallel lines with optional caps and fills.",
        "point" => "Places a point object. The point style (PDMODE) controls how points appear.",
        "divide" => "Places point objects or blocks at equal intervals along an object.",
        "measure" => "Places point objects or blocks at measured intervals along an object.",
        "region" => "Converts closed shapes (polylines, circles, ellipses, splines) into region \
                     objects for Boolean operations.",
        "boundary" => "Creates a polyline or region from an enclosed area.",
        "revcloud" => "Draws a revision cloud to mark areas that have changed.",
        "donut" => "Draws a filled ring (donut) with an inside and outside diameter.",
        "helix" => "Draws a 3D helix (spiral) with a base radius, top radius, height and turns.",
        "mtext" => "Creates multiline text with formatting (bold, italic, fonts, stacking, \
                    columns). The in-place editor opens when you define the text boundary.",
        "text" => "Creates single-line text. Pick an insertion point, set the height and rotation, \
                   then type. Press Enter twice to finish.",
        "table" => "Inserts a table with rows and columns. Specify the number of columns and rows \
                    or pick a size on screen.",
        "hatch" => "Fills an enclosed area with a pattern or solid colour. Pick an internal point \
                    or select boundary objects; choose the pattern, scale and angle.",
        "gradient" => "Fills an enclosed area with a colour gradient.",

        // ── Editing ───────────────────────────────────────────────────────────────
        "erase" => "Deletes selected objects from the drawing.",
        "copy" => "Duplicates selected objects at a displacement you specify. Pick a base point \
                   and one or more destination points; press Enter to finish.",
        "move" => "Moves selected objects. Pick a base point and a destination point.",
        "rotate" => "Rotates selected objects about a base point by an angle you specify. \
                     The Reference option lets you align to a known angle.",
        "scale" => "Resizes selected objects uniformly about a base point by a scale factor. \
                    The Reference option lets you scale to a known length.",
        "mirror" => "Creates a mirrored copy of selected objects across a line you pick.",
        "offset" => "Creates a parallel copy of a line, arc, circle or polyline at a specified \
                     distance or through a point.",
        "trim" => "Trims objects to a cutting edge. Select cutting edges first, then pick the \
                   parts to remove. In quick mode (the default) all objects act as edges.",
        "extend" => "Extends objects to meet a boundary edge. Select boundary edges first, then \
                     pick the objects to extend.",
        "fillet" => "Rounds the corner between two lines, arcs or polyline segments with an arc \
                     of a specified radius. A radius of 0 produces a sharp corner.",
        "chamfer" => "Bevels the corner between two lines with a straight line. Specify two \
                      distances or a distance and an angle.",
        "stretch" => "Stretches objects that cross a selection window. Objects fully inside the \
                      window move; objects crossing the window boundary stretch.",
        "array" => "Creates copies of objects in a rectangular, polar or path pattern.",
        "arrayrect" => "Creates a rectangular array with rows, columns and optional levels.",
        "arraypolar" => "Creates a polar (circular) array about a centre point.",
        "arraypath" => "Creates copies along a path (line, arc, polyline or spline).",
        "break" => "Splits an object at one or two points, removing the segment between them.",
        "join" => "Combines collinear lines, coplanar arcs or polyline segments into one object.",
        "explode" => "Breaks a compound object (block, polyline, hatch, dimension, mline) into \
                      its individual parts.",
        "pedit" => "Edits polylines and 3D meshes. Options include Open, Close, Join nearby \
                    segments, change Width, Fit curve, Spline, Decurve, and edit individual \
                    vertices.",
        "matchprop" => "Copies properties (layer, color, linetype, lineweight, plot style and \
                        others) from a source object to destination objects.",
        "chprop" => "Changes properties of selected objects: color, layer, linetype, lineweight, \
                     transparency, linetype scale, thickness and material.",
        "undo" => "Reverses the most recent operation. Repeat to step back further. Mark and Back \
                   let you undo to a saved point.",
        "redo" => "Reverses the last UNDO. Only available immediately after an UNDO.",
        "oops" => "Restores the most recently erased objects, even after other commands.",
        "draworder" => "Changes the display order of overlapping objects (bring to front, send to \
                        back, above/below a reference object).",
        "align" => "Moves, rotates and scales objects in one step by mapping source points to \
                    destination points (1, 2 or 3 pairs).",
        "lengthen" => "Changes the length of an arc or open curve by a delta, a percentage, a \
                       total or dynamically.",

        // ── Blocks ────────────────────────────────────────────────────────────────
        "block" => "Defines a block from selected objects. Give it a name, pick a base point, and \
                    choose whether to keep, convert or delete the originals.",
        "-block" => "Command-line version of BLOCK (no dialog).",
        "insert" => "Inserts a block reference into the drawing. Specify the name, insertion \
                     point, scale and rotation.",
        "-insert" => "Command-line version of INSERT (no dialog).",
        "wblock" => "Writes a block or selected objects to a new drawing file.",
        "bedit" => "Opens the Block Editor so you can modify a block definition in place.",
        "bclose" => "Closes the Block Editor and saves or discards changes.",
        "refedit" => "Edits a block or xref in place without opening the Block Editor.",
        "refclose" => "Closes an in-place reference edit and saves or discards changes.",
        "xref" => "Manages external references (xrefs): attach, detach, reload, unload, bind.",
        "xattach" => "Attaches an external drawing as an xref.",
        "xbind" => "Binds named items (layers, styles, blocks) from an xref into the drawing.",
        "purge" => "Removes unused named items (blocks, layers, linetypes, text styles, dimension \
                    styles, materials and others) from the drawing.",
        "burst" => "Explodes blocks and converts their attributes to text objects (unlike EXPLODE, \
                    which discards attribute values).",

        // ── Layers ────────────────────────────────────────────────────────────────
        "layer" => "Opens the Layer Properties Manager where you create, rename, delete and \
                    configure layers (color, linetype, lineweight, transparency, plot, freeze, \
                    lock, description).",
        "-layer" => "Command-line version of LAYER.",
        "laycur" => "Moves selected objects to the current layer.",
        "layiso" => "Isolates selected layers: turns off or fades all others.",
        "layuniso" => "Restores layers that were hidden by LAYISO.",
        "layoff" => "Turns off the layer of a selected object.",
        "layon" => "Turns on all layers.",
        "layfz" => "Freezes the layer of a selected object.",
        "laythw" => "Thaws all layers.",
        "laylck" => "Locks the layer of a selected object so its objects cannot be edited.",
        "layulk" => "Unlocks the layer of a selected object.",
        "laydel" => "Deletes a layer and moves its objects to another layer.",
        "laymrg" => "Merges one layer into another and deletes the source layer.",
        "laymch" => "Moves objects to the layer of a chosen object.",

        // ── Views ─────────────────────────────────────────────────────────────────
        "zoom" => "Changes the view magnification. Options include All, Center, Dynamic, Extents, \
                   Previous, Scale, Window, Object and Realtime.",
        "pan" => "Shifts the view without changing the magnification. Drag to pan, or specify \
                  a displacement.",
        "regen" => "Regenerates the entire drawing, recalculating the display of every object.",
        "regenall" => "Regenerates all viewports.",
        "redraw" => "Refreshes the display of the current viewport.",
        "view" => "Saves and restores named views.",
        "3dorbit" => "Rotates the 3D view interactively. Drag to orbit around the target.",
        "3dwalk" => "Walks through a 3D model at eye level using keyboard and mouse controls.",
        "3dfly" => "Flies through a 3D model in any direction.",
        "vscurrent" => "Sets the visual style of the current viewport (2D Wireframe, Conceptual, \
                        Realistic, Shaded, X-Ray and others).",
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
        "-dimstyle" => "Command-line dimension style management (Apply current overrides to \
                        selected dimensions, create, rename, delete styles).",
        "qdim" => "Creates a quick dimension from selected geometry: continuous, baseline, \
                   ordinate, staggered, radius or diameter.",
        "dim" => "Starts the DIM command for continuous dimensioning with modes (linear, aligned, \
                  angular, radius, diameter, ordinate, baseline, continue, distribute).",
        "qleader" => "Creates a leader with text annotation.",
        "mleader" => "Creates a multileader: an arrow with a text or block label, formatted by an \
                      mleader style.",
        "mleaderstyle" => "Opens the Multileader Style Manager.",

        // ── Annotation ────────────────────────────────────────────────────────────
        "style" => "Opens the Text Style dialog to create and modify text styles (font, height, \
                    width factor, oblique angle).",
        "linetype" => "Opens the Linetype Manager to load and set linetypes.",
        "ltscale" => "Sets the global linetype scale factor.",
        "color" => "Sets the current colour for new objects.",

        // ── Inquiry ───────────────────────────────────────────────────────────────
        "dist" => "Measures the distance and angle between two points.",
        "area" => "Calculates the area and perimeter of a region, polyline or sequence of points. \
                   Add and Subtract options let you combine multiple areas.",
        "list" => "Displays detailed information about selected objects (type, layer, color, \
                   position, geometry).",
        "id" => "Reports the coordinates of a picked point.",
        "measuregeom" => "Measures distance, radius, angle or area interactively, with Quick mode \
                          for one-pick nearest-geometry measurements.",
        "massprop" => "Calculates mass properties (area, perimeter, centroid, moments of inertia) \
                       for regions and solids.",

        // ── Selection ─────────────────────────────────────────────────────────────
        "select" => "Builds a selection set using Window, Crossing, WPolygon, CPolygon, Fence, \
                     Last, Previous, All or a pick.",
        "selectall" => "Selects every object in the current space.",
        "qselect" => "Opens Quick Select to select objects by property criteria.",
        "selectsimilar" => "Selects all objects that share properties with the selected object \
                            (layer, color, linetype, name, style or block name).",
        "filter" => "Creates or applies a named selection filter.",

        // ── Settings ──────────────────────────────────────────────────────────────
        "osnap" => "Sets running object snap modes (endpoint, midpoint, center, quadrant, \
                    intersection, extension, perpendicular, tangent, nearest, node, insert, \
                    none).",
        "snap" => "Constrains the cursor to a grid of snap points at a specified spacing.",
        "grid" => "Displays a grid of dots or lines at a specified spacing as a visual reference.",
        "ortho" => "Constrains cursor movement to horizontal and vertical directions.",
        "limits" => "Sets the drawing limits (the area covered by the grid).",
        "units" => "Sets the format and precision of coordinates and angles.",
        "options" => "Opens the Options dialog for application-wide settings (display, files, \
                      system, user preferences, drafting, 3D, paths).",

        // ── 3D Solids ─────────────────────────────────────────────────────────────
        "box" => "Creates a 3D solid box.",
        "sphere" => "Creates a 3D solid sphere.",
        "cylinder" => "Creates a 3D solid cylinder.",
        "cone" => "Creates a 3D solid cone.",
        "torus" => "Creates a 3D solid torus (ring).",
        "wedge" => "Creates a 3D solid wedge.",
        "pyramid" => "Creates a 3D solid pyramid.",
        "extrude" => "Creates a 3D solid or surface by extruding a 2D profile along a path or \
                      by a height.",
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
        "pagesetup" => "Opens the Page Setup dialog for the current layout (printer, paper size, \
                        plot area, scale, plot style table).",
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
        "eattedit" => "Opens the Enhanced Attribute Editor for a block reference.",
        "battman" => "Opens the Block Attribute Manager to edit attribute definitions in blocks.",
        "dataextraction" => "Extracts object and attribute data to a table or external file.",
        "field" => "Inserts a field that updates automatically (date, file name, area, sheet \
                    number and others).",

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
        "secureload" => "Controls which locations are trusted for loading scripts and LISP files.",
        "appload" => "Loads an AutoLISP (.lsp) application into the current session.",

        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command has a help page that can be generated.
    #[test]
    fn every_command_has_a_help_page() {
        for spec in crate::cmd::command_specs() {
            let page = spec.help_page();
            assert!(!page.id.is_empty(), "command {:?} has no id in its help page", spec.id);
            assert!(!page.label.is_empty(), "command {:?} has no label in its help page", spec.id);
        }
    }

    /// A help page lists the command's aliases.
    #[test]
    fn help_page_lists_aliases() {
        for spec in crate::cmd::command_specs() {
            let page = spec.help_page();
            assert_eq!(
                page.aliases.len(),
                spec.aliases.len(),
                "command {:?}: help page aliases count mismatch",
                spec.id,
            );
            for (i, alias) in spec.aliases.iter().enumerate() {
                assert_eq!(
                    page.aliases[i],
                    alias.to_ascii_uppercase(),
                    "command {:?}: alias {:?} not in help page",
                    spec.id,
                    alias,
                );
            }
        }
    }

    /// A help page includes the JSON params when present.
    #[test]
    fn help_page_includes_params() {
        for spec in crate::cmd::command_specs() {
            let page = spec.help_page();
            assert_eq!(
                page.params, spec.params,
                "command {:?}: help page params mismatch",
                spec.id,
            );
        }
    }

    /// Commands without help text: a report for coverage tracking.
    #[test]
    fn commands_without_help_report() {
        let missing = crate::cmd::commands_without_help();
        let total = crate::cmd::command_specs().len();
        let covered = total - missing.len();
        eprintln!(
            "Help text coverage: {covered}/{total} commands ({:.0}%)",
            covered as f64 / total as f64 * 100.0
        );
        if !missing.is_empty() {
            eprintln!("Commands without hand-written help text ({}):", missing.len());
            for id in &missing {
                eprintln!("  - {}", id.to_ascii_uppercase());
            }
        }
        // At least 100 commands should have help text.
        assert!(
            covered >= 100,
            "Only {covered} commands have help text; at least 100 are required",
        );
    }

    /// The lookup function returns non-empty text for known commands.
    #[test]
    fn lookup_returns_text_for_known_commands() {
        assert!(!lookup("line").is_empty());
        assert!(!lookup("circle").is_empty());
        assert!(!lookup("copy").is_empty());
        assert!(lookup("nonexistent_command_xyz").is_empty());
    }

    /// The HELP command returns a page for a known command.
    #[test]
    fn help_page_for_known_command() {
        let spec = crate::cmd::find_by_id_or_alias("line").expect("LINE should exist");
        let page = spec.help_page();
        assert_eq!(page.id, "LINE");
        assert!(!page.help_text.is_empty(), "LINE should have help text");
        assert!(page.aliases.contains(&"L".to_string()), "LINE should have alias L");
    }

    /// find_by_id_or_alias works for aliases.
    #[test]
    fn find_by_alias() {
        let spec = crate::cmd::find_by_id_or_alias("c").expect("C should find CIRCLE");
        assert_eq!(spec.id, "circle");
        let spec = crate::cmd::find_by_id_or_alias("l").expect("L should find LINE");
        assert_eq!(spec.id, "line");
    }
}
