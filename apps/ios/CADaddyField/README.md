# CADaddy Field (iOS / iPadOS)

A native iPhone and iPad app that opens CADaddy AR packages (from `bim.export.ar`), places the model in
the real room, and shows each object's data on tap. In the room you can mark objects installed, missing,
moved or extra, then export the marks as one JSON file for CADaddy.

- Apple frameworks only: SwiftUI, ARKit, RealityKit, Foundation, plus simd, Compression (zip inflate),
  CryptoKit (package hash) and PhotosUI (photo picker). No third-party packages.
- No accounts, analytics or cloud. The only network call is **Update**, and only when you tap it: a
  GET of `manifest.json` and the changed package files from the office address you entered, on the
  local network only (`NSAllowsLocalNetworking` and `NSLocalNetworkUsageDescription` in Info.plist,
  no blanket ATS exception). There are no background checks. Packages, marks, photos and saved rooms
  stay in the app's Application Support folder, which is excluded from backups, and the app works
  fully offline.
- Bundle id `dev.aicodeforge.cadaddyfield`, display name "CADaddy Field", author AICodeForge.
- **Minimum iOS 17.0.** The LiDAR APIs used here (scene reconstruction `.mesh` and RealityKit
  scene-understanding occlusion) exist from iOS 13.4. The UI needs iOS 17 for SwiftUI `.inspector`,
  `ContentUnavailableView` and the two-argument `onChange`. Every LiDAR iPhone and iPad (iPhone 12 Pro
  and later, iPad Pro 2020 and later) runs iOS 17. Devices without LiDAR work too, without occlusion.
- iPhone and iPad layouts: a split view on iPad that collapses to a stack on iPhone. The object card
  is an inspector, a side panel on iPad and a sheet on iPhone.

## Build and test

Needs Xcode 16 or later (the project uses folder-synchronised groups). These are the exact commands
used for this build:

```sh
cd apps/ios/CADaddyField

# Unit tests on the iOS simulator (install a runtime first if none: xcodebuild -downloadPlatform iOS)
xcodebuild -project CADaddyField.xcodeproj -scheme CADaddyField \
  -destination 'platform=iOS Simulator,name=iPhone 17' test

# Simulator build
xcodebuild -project CADaddyField.xcodeproj -scheme CADaddyField \
  -destination 'generic/platform=iOS Simulator' build

# Device build without signing (no team is set in the project)
xcodebuild -project CADaddyField.xcodeproj -scheme CADaddyField -configuration Release \
  -destination 'generic/platform=iOS' CODE_SIGNING_ALLOWED=NO build
```

To install on a phone, open the project in Xcode and pick a signing team under Signing & Capabilities.

`apps/ios` is excluded from the Cargo workspace (`Cargo.toml` `exclude`), so `cargo` doesn't look at it.

### Layout

| Folder | What |
|---|---|
| `FieldCore/` | Foundation-only logic shared by the app and the tests: GLB reader/writer, sidecar, zip reader, package library, alignment solver, marks, units, caps. |
| `CADaddyField/` | The app: SwiftUI screens (`UI/`), RealityKit/ARKit (`AR/`), entry point (`App/`). |
| `CADaddyFieldTests/` | XCTest. It compiles `FieldCore/` directly, with no host app. |
| `Support/Info.plist` | Document types (.glb, .usdz, .zip, folder) and the imported glTF-binary type. |

### Debug launch arguments (Debug builds only)

- `-demo library|elements|card|marks|plan|preview` seeds the synthetic sample room and opens that
  screen. Used for the screenshots.
- `-demorooms N` makes the seeded sample N rooms (a large package).
- `-fpslog` makes the 3D preview orbit by itself and print `FIELD-PERF` lines: build time, and
  frames per second over 10 s. Capture them with `xcrun simctl launch --console-pty <device>
  dev.aicodeforge.cadaddyfield -demo preview -fpslog`.

## Opening packages

From the Files app (+ › Open from Files…), the share sheet, AirDrop, or "Open in" you can open:

- a `.glb` (glTF 2.0 binary). This gives full per-object metadata. A sibling `<name>.json`,
  `<name>.anchor.json`, `<name>_anchor.json` or `<name>-anchor.json` is picked up when the system
  gives access to it.
- a `.usdz`. RealityKit loads it. Per-object metadata comes from the sidecar's `elements` list,
  matched by node name. Without that list the card shows only what the node name says, and the
  library notes this.
- a folder or `.zip` holding the model and its sidecar `.json`. A `.glb` is preferred over a `.usdz`.
  The sidecar is `<model>.json`, else the only `.json`, else one named like `anchor`/`sidecar`.
  Stored and deflate zips are supported. Zip64, encrypted and split zips are refused. Entries that
  escape the folder (`..`, absolute paths) and macOS metadata are ignored.

Malformed or oversized packages are refused with a message, and nothing is left in the library. The
caps are in `FieldCore/Limits.swift`: 400 MB model, 250k nodes, 3 M triangles, 4 M vertices per
primitive, node depth 64, cycles refused, 10k zip entries, 1 GB expanded.

Large packages build progressively: devices first, then the building shell, in batches of 150
objects, with a progress bar.

## Package format (what the app reads)

The contract is Q6207 `arexport`. Until that export lands, the app is built against the layout the
queue item specifies, using its own synthetic sample (`FieldCore/SamplePackage.swift`, also under
+ › Add sample room).

- **Frame:** metres, +Y up. The anchor is at the origin and the finished floor at y = 0.
- **One node per element.** Metadata goes in the glTF node `extras` (or `extras.cadaddy`). Keys are
  read leniently:
  - `id` (or `element_id`/`elementId`/`guid`)
  - `kind`, `tag`, `circuit`, `panel`
  - `mounting_height_m`, or `mounting_height` as metres or as text like `18"`; values over 30 are taken as inches
  - `catalog` (or `product`), `trade` (or `system`), `source`

  Other keys show on the card as extra rows. A node's child meshes belong to it. Mesh nodes without
  metadata become objects named after the node.
- **Colours:** the material `baseColorFactor` as exported (by trade). Alpha < 1 with
  `alphaMode: BLEND` is translucent.
- **Instancing:** shared meshes (many nodes, one mesh) are built once. `EXT_mesh_gpu_instancing` is
  expanded.
- **Context:** kinds wall/floor/slab/ceiling/roof/door/window/column/beam/room/space, or trade
  `context`, are context. They can be shown, made translucent or hidden, and are not tappable.
- **Sidecar `.json`.** Keys may sit at the top level or under `cadaddy`:

```json
{
  "format": "cadaddy-ar", "version": 1, "title": "Room 1", "model": "Room 1.glb",
  "units": "m", "up_axis": "Y",
  "anchor": { "kind": "room_corner | grid_intersection | named_point", "name": "…",
              "plan_x": 0.0, "plan_y": 0.0, "plan_units": "ft", "level": "…", "room": "…" },
  "points":   [ { "name": "Far corner", "position": [4.0, 0.0, -7.6] } ],
  "elements": [ { "id": "…", "node": "Receptacle_R1", "kind": "…", "tag": "…", "circuit": "…",
                  "panel": "…", "mounting_height_m": 0.4572, "catalog": "…", "trade": "…", "source": "…" } ]
}
```

  - Only `anchor` is needed for alignment.
  - `points` (optional) are extra snap targets for two-point alignment.
  - `elements` (optional) fills metadata for USDZ, or for GLB nodes that lack it.
  - An anchor in glTF `asset.extras.cadaddy.anchor` (or the scene's `extras`) is used when there is
    no sidecar.
  - Up axes other than Y are refused.

## Update (pull the latest packages from the office)

The office runs `bim.export.ar.manifest {dir}` on its folder of AR exports and serves that folder
with any static web server on the LAN. The format and the rules are in `docs/ar-export.md`,
"Serving exports to the field app".

- **Update** sits in the Library toolbar on iPhone and iPad. The first tap asks for the office
  address, a full URL that may carry a private path. It is stored on the device; change it from
  + ▸ Office address, or with a long press on Update.
- A tap fetches the manifest, then downloads only the packages whose model or sidecar hash changed.
  It shows progress, then the result: N updated, N new, N unchanged, N failed, with the reason for
  each failure.
- **Every file is checked before anything is replaced:** its size, its SHA-256, and that the model
  opens. The package folder is then swapped in one step. Any failure (no Wi-Fi, server unreachable,
  bad JSON, a `../` or absolute name, an oversized or wrong file, a hash mismatch) gives a plain
  message, and the old package keeps working.
- **Marks and photos carry over** by element id, or by the device's stable `devid` after a rebuild.
  A mark whose element is gone stays and shows "element no longer in model". Exported marks carry
  the `package_version` they were made against.
- **The saved room** is kept when the anchor didn't move. When it moved, the old anchor shows as a
  grey cross and the app asks you to align again.

Debug builds only: `-updateserver URL` presets the address, and `-autoupdate` taps Update once at
launch. The simulator end-to-end run uses them.

## Using it in the room

1. **Align**, either way:
   - **Quick:** tap the real floor at the anchor (the red cross on the model and the plan), then
     drag sideways to turn the model until it lines up with a wall, then tap Done.
   - **Two-point:** pick point A on the plan. It snaps to the anchor, the named points, and wall
     corners at floor level. Tap A's real spot on the floor, then do the same for B. The app solves
     the rotation about vertical plus the translation, and puts the model floor on the floor ARKit
     detected. It shows the residual in inches. Picks under 1 ft apart, or picks whose distances
     disagree by more than 10%, are refused.
2. **Nudge:** move the model 1/8" at a time along its own X, Y and Z, or turn it 0.1° at a time.
   Holding a button repeats it.
3. **Layers:** per-trade and per-kind toggles; walls on, translucent or off; LiDAR occlusion; and
   mounting-height labels (up to 300).
4. **Tap an object** to open its card: kind, tag, circuit and panel, mounting height above the
   finished floor, catalog, trade, element id and source. The card also shows two live values: the
   distance from the phone, and the height above the detected real floor.
5. **Mark** the object installed, missing, moved or extra, with an optional note and photo (from
   the library, or a snapshot of the AR view). "Moved" then asks you to tap the spot where it is
   actually installed.
6. **Save room:** after aligning, the app saves the ARKit world map with an anchor that holds the
   alignment. If the map isn't ready yet, it saves as soon as it is. When you reopen the package in
   the same room, it relocalises and restores the alignment. If it can't within 25 s, it says so
   and asks you to align again.

## Marks export (schema `cadaddy-field-marks`, version 1)

Marks › Export writes `<package> marks.json` and opens the share sheet.

```json
{
  "schema": "cadaddy-field-marks",
  "version": 1,
  "app": "CADaddy Field 0.1.0",
  "exported": "2026-11-05T19:52:10.123Z",
  "package": { "name": "…", "model_file": "model.glb", "model_sha256": "…",
               "version": { "model_sha256": "…", "exported": "2026-11-06T04:00:00Z" },
               "anchor": { "kind": "room_corner", "name": "…", "plan_x": 0, "plan_y": 0, "plan_units": "ft", "level": "…" } },
  "alignment": { "method": "quick | two_point | relocalized | none", "residual_in": 0.42 },
  "marks": [
    {
      "id": "UUID",
      "element_id": "E-1015", "element_kind": "Receptacle", "element_tag": "R6",
      "status": "installed | missing | moved | extra",
      "note": "optional text",
      "time": "2026-11-05T19:50:01.250Z",
      "device_pose_world": [16 floats], "device_pose_model": [16 floats],
      "point_world": [x, y, z], "point_model": [x, y, z],
      "photo": "file name", "photo_data": { "name": "…", "mime": "image/jpeg", "data_base64": "…" },
      "package_version": { "model_sha256": "…", "exported": "…" },
      "element_gone": true, "element_id_was": "E-0998"
    }
  ]
}
```

- Times are ISO 8601 UTC with milliseconds.
- Poses are column-major 4×4. `*_world` is the AR session frame, which differs per session. `*_model`
  is the export frame (metres, Y up, the anchor at the origin), which is the one CADaddy should use.
- `point_*` is present only for "moved" marks that were tapped in AR.
- Photos are JPEG, at most 1600 px and 4 MB, embedded as base64.
- Optional fields are left out when empty. `element_gone` and `element_id_was` are written only by
  Update: the first when the mark's element is no longer in the model, the second when the mark was
  re-linked to the same device under a new id.
- Importing marks into CADaddy is a later item. `MarksExport.decode` refuses files with another
  schema or a newer version.

## Not in this build

- LiDAR auto-fit to scanned walls
- Sync back into CADaddy (pushing marks to the office is a later item; Update only pulls)
- Editing geometry
- Accounts
- App Store submission
- Signing: the team is left unset
