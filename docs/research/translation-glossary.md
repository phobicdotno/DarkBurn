# Translation glossary

For whoever translates DarkBurn's interface. It says what each laser and drawing term means in
DarkBurn, so that one word is used for it throughout a language. It holds no translations: every
language picks its own words, the ones its laser users already know.

## How a language is added

- The text lives in `assets/locales/<code>.json`: one object, key to text, with the keys of
  `en.json`. `en.json` is made from the program by a test and is the list of everything to
  translate. Copy the keys you translate; leave out the ones you have not done yet.
- A key that is missing (or empty) shows its English text, so a file can start small.
- The program picks up every file in that folder by itself. The languages it knows are in
  `apps/darkburn/src/languages.rs`, one line each (code, the language's own name, its script); a
  file with a code that has no line there fails a test. The 22 codes planned so far already have
  their line: ar, cs, da, de, el, es-ES, es-LA, fi, fr, hu, it, ja, ko, nl, pl, pt, ru, sl, sv,
  tr, zh-CN, zh-TW.
- A language shows in the Language menu once its file is there.
- `cargo test -p darkburn i18n` checks every file against the rules below
  (`every_locale_file_keeps_the_rules_of_the_english_text`), and `cargo test -p darkburn
  language_fonts` that every character of a file has a glyph in the fonts picked for it.

## Rules the checker holds every file to

- Valid JSON, one flat object, every value a text, no key that `en.json` lacks, no empty text.
- Placeholders: `{name}` (letters, digits and `_` between braces, `{}` too) is filled in by the
  program. Keep every placeholder of the English text, unchanged; the order may change. (No text
  uses one yet.)
- Spaces at the start or the end of a text are there for a reason (a unit after a number, say):
  keep them exactly as English has them, and add none.
- A text that ends with a colon in English ends with one in the translation (`:` or the
  full-width `：`), and the other way round.
- A text that ends with `...` in English (a command that opens a dialog first) ends with `...`
  or `…`, and the other way round.
- Shortcut marks: an `&` right before a letter marks the key for that menu item. Keep as many as
  English has (a different letter is fine). `&&` and `& ` are a plain ampersand.
- Key names stay as they are, as many times as in English: Ctrl, Shift, Alt, Cmd, Esc, F1 to
  F24. They name keys printed on the keyboard.
- Leave these names as they are: DarkBurn, LightBurn, G-code, GRBL, Ruida, Gweike, DXF, SVG, PDF,
  PNG, QR, DPI, mm, mm/s, mm/min.

## Scripts and fonts

- The interface font, IBM Plex Sans, draws Latin (all the European languages in the list),
  Greek and Cyrillic. Nothing else is needed for them.
- Japanese, Korean and Simplified and Traditional Chinese are drawn with fonts the computer
  already has; the program adds no font files. It looks for the usual system font of each script
  first (Windows: Yu Gothic / Meiryo, Malgun Gothic, Microsoft YaHei, Microsoft JhengHei; macOS:
  Hiragino, Apple SD Gothic Neo, PingFang; Linux: Noto Sans CJK, Source Han Sans, WenQuanYi,
  Nanum and others), then any font that has the characters. A test checks that every character
  of a file has a glyph in the fonts picked for it; on Linux, install `fonts-noto-cjk` for it.
  The full list per script is `Script::preferred_fonts` in `languages.rs`.
- **Arabic is not offered.** egui 0.37 (the toolkit the interface is drawn with) shapes the
  letters of each word, so they join, but it has no bidirectional text: it lays the runs of a
  line out left to right in the order they are written. In Arabic text the spaces and digits
  come from the interface font, so each word is a run of its own, and a sentence comes out with
  its words in reverse order; alignment and the layout of panels stay left to right as well.
  The code says as much (epaint's text layout: "need script-aware splitting once RTL/bidi
  support is added"; its Unicode handling: "heed bidi characters" is a to-do). The table keeps
  Arabic's line, marked right to left, and the menu leaves every right-to-left language out. A
  test lays out a two-word Arabic phrase and fails once egui puts the first word on the right:
  then Arabic can be looked at again.

## Menus

One line per menu and submenu name (keys `menu.*`).

- **File**: projects and files: new, open, save, import, export, print, preferences, quit.
- **Edit**: undo, the clipboard, selecting, and editing the selected shapes; also the settings.
- **Tools**: the drawing and editing tools (as in the toolbar), measuring, tracing, booleans.
- **Arrange**: grouping, order, alignment, flipping, rotating, arrays and moving shapes.
- **Laser Tools**: tests and setup that need the laser: material, focus and interval tests,
  rotary, camera calibration, print and cut.
- **Window**: which panels and toolbars show, and the window layout.
- **Language**: the interface language. Each language is named in itself, never translated.
- **Help**: help, the documentation, updates, debug log, about.
- **Align**: line up the selected shapes on one edge or centre.
- **Distribute**: space the selected shapes out evenly.
- **Dock**: push the selection against its neighbours or the page edge in one direction.
- **Image Options**: commands for a selected bitmap image (crop, mask, replace, refresh).
- **Move Laser to Selection**: move the laser head to a point of the selection's outline box.
- **Move Selected Objects**: move the selection to a point of the page (a corner, the centre).
- **Print and Cut**: line up a cut with a printed sheet by two target marks.
- **View Style**: how shapes are drawn on screen: wireframe or filled, coarse or smooth.
- **Virtual Array**: a set of copies that follow one original as it is edited.
- **Panels**: the panels (Cuts / Layers, Laser, Move and the others) that can be shown or hidden.
- **Recent Projects**: the project files opened last.
- **Toolbars**: the toolbars that can be shown or hidden.

Ribbon tab names (keys `ribbon.tab.*`): Home (the everyday commands, not the machine's home
position), Arrange, Modify (changing shapes), Text, Modes.

## Laser and machine terms

- **Laser**: the machine as a whole, or its beam. The Laser panel is where a job is started.
- **Job**: everything sent to the laser in one go: the shapes, their order and their settings.
- **Cut** (noun and verb): a pass of the beam along a line that goes through the material; also
  the cut settings of a layer ("Cuts / Layers"). Keep it apart from the clipboard's Cut; if the
  language needs two words, the menu Edit > Cut is the clipboard one.
- **Line** (mode): the beam follows the outline of a shape, like a pen.
- **Fill** (mode): the beam sweeps back and forth over the inside of a closed shape to engrave an
  area.
- **Image** (mode): a bitmap picture engraved dot by dot, its greys turned into dots or power.
- **Engrave**: mark the surface without cutting through.
- **Layer**: a group of shapes that share one colour and one set of cut settings (mode, speed,
  power, passes). Not a drawing layer that stacks shapes.
- **Sub-layer**: a further set of settings for the same shapes of a layer, run after the first.
- **Power**: the laser's output, as a percentage of its maximum. Min/Max power are the lower and
  upper limits within one cut.
- **Speed**: how fast the head moves while it burns, in mm/s or mm/min.
- **Pass**: one run over the shapes. A layer can be cut in several passes.
- **Interval**: the distance between two neighbouring lines of a fill or an image.
- **Kerf**: the width of material the beam burns away; Kerf offset moves the line by half of it
  so parts come out the right size.
- **Overscan**: extra travel past the end of each fill line, so the head is at full speed while
  it burns.
- **Air assist**: a jet of air at the lens that blows smoke and flame away from the cut.
- **Frame**: run the head (without burning, or at low power) around the outline of the job, to
  see where it will land on the material.
- **Jog**: move the head by hand, by a set step, with buttons or keys.
- **Home** (machine): drive the axes to their end switches, so the machine knows where the head
  is. Not the ribbon's Home tab.
- **Origin**: the point the job's coordinates count from; the "job origin" is the point of the
  job that sits at the head's position.
- **Start point**: where the beam starts on a closed shape.
- **Cut direction**: which way round a shape the beam goes.
- **Tab** (on a shape): a short gap left uncut so the part stays held in the sheet. Not a tab of
  a panel or a dialog.
- **Focus**: the distance between lens and material at which the beam is smallest; Focus test
  burns a row of lines at different heights to find it.
- **Material test**: a grid of squares burned at different speeds and powers, to pick settings.
- **Cut library** / **Material library**: saved cut settings for materials and thicknesses.
- **Rotary**: an attachment that turns round objects (glasses, tumblers) under the beam.
- **Galvo**: a laser that steers the beam with mirrors instead of moving a head; fast marking.
- **Camera**: a camera over the bed that shows the material, to place the design on it.
- **Print and cut**: aligning a job to marks on a printed sheet.
- **G-code**: the text commands most hobby lasers run; keep the name.
- **Device** / **Machine**: the laser DarkBurn is set up to drive, with its settings.
- **Console**: the panel that shows the messages to and from the machine.
- **Preview**: a picture of the job as it will run, with the travel between shapes.
- **Nudge**: move the selection a small set step with the arrow keys.

## Drawing and editing terms

- **Shape** / **Object**: one thing on the page: a path, a rectangle, an ellipse, text, an image.
- **Path**: a shape made of line and curve segments, open or closed.
- **Node**: a point on a path where two segments meet; Edit Nodes moves and changes them.
- **Weld**: merge overlapping shapes into one outline, removing the lines inside.
- **Boolean**: union (add), difference (subtract) and intersection of two closed shapes.
- **Offset**: a new outline at a set distance outside or inside a shape.
- **Trace** (image): turn the edges of a bitmap image into paths.
- **Dither**: turning the greys of an image into a pattern of black dots, as the laser burns
  only on or off (methods such as Jarvis, Stucki, Floyd-Steinberg keep their names).
- **Bitmap**: an image made of pixels, as opposed to shapes made of lines.
- **Group** / **Ungroup**: treat several shapes as one, and back.
- **Array**: copies of a shape in a grid, a circle or along a path.
- **Align**, **Distribute**, **Dock**: see the menus above.
- **Flip** / **Mirror**: turn a shape over, horizontally or vertically or across a line.
- **Snap**: let the pointer jump to the grid, to points of shapes or to guides.
- **Grid**: the lines on the page in a set spacing.
- **Lock**: keep a shape from being moved or changed by accident.
- **Crop** / **Mask**: show only part of an image, the part inside a path.
- **Variable text**: text filled in at cut time from a serial number, a date or a list.
- **Selection**: the shapes picked to work on; the outline box around them.
- **Page**: the work area on screen, the size of the laser's bed.
- **Panel**: a dockable part of the window (Cuts / Layers, Laser, Move, Library and the others).
- **Toolbar**, **Ribbon**: the rows of command buttons; the ribbon has tabs.
- **Hotkey**: a keyboard shortcut for a command.
- **Preferences** / **Settings**: the program's options (Settings), saved per user.
