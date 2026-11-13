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
- Key names stay as they are, as many times as in English: Ctrl, Cmd, Esc and F1 to F24
  anywhere, Shift and Alt in a key combination (next to a `+`, as in `Alt+Drag`). They name keys
  printed on the keyboard. On their own, "Shift" and "Alt" are not checked, because "Alt" is an
  ordinary word in Norwegian and Danish ("all") and German ("old").
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
  The fonts are worked out on a thread of their own, so the window does not wait; on a computer
  without a font for the script the search gives up after a couple of characters and the text
  shows as boxes. The faces for the language names in the Language menu are added only while the
  menu is open.
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

## German (de)

Formal "Sie" in help texts; commands as infinitives ("Formen versetzen"). Key names stay as in
English: Ctrl, Shift, Esc (not Strg, Umschalt). Panel labels that are not translated yet are
quoted „like this“.

- Job: Auftrag. Cut (laser): Schnitt, schneiden. Cut (clipboard): Ausschneiden.
- Layer: Ebene; sub-layer: Unterebene. Line / Fill / Image: Linie / Füllen / Bild. Engrave: gravieren.
- Power: Leistung. Speed: Geschwindigkeit. Pass: Durchgang. Interval: Intervall. Kerf: Schnittbreite.
- Overscan: Überlauf. Air assist: Luftunterstützung. Frame: Rahmen fahren / Umriss abfahren.
- Jog: verfahren. Home: Referenzfahrt. Origin: Nullpunkt. Start point: Startpunkt.
- Cut direction: Schnittrichtung. Tab: Haltesteg. Focus test: Fokustest. Material test: Materialtest.
- Cut library / Material library: Schnittbibliothek / Materialbibliothek. Rotary: Drehachse.
- Camera: Kamera. Print and cut: Drucken und Schneiden. Device / Machine: Gerät / Maschine.
- Console: Konsole. Preview: Vorschau. Nudge: Pfeiltasten-Schritt.
- Shape / Object: Form / Objekt. Path: Pfad. Node: Knoten. Weld: verschmelzen.
- Boolean union / difference / intersection: Boolesche Vereinigung / Differenz / Schnittmenge.
- Offset: Versatz (command: Formen versetzen). Trace: nachzeichnen. Bitmap: Bitmap.
- Group / Ungroup: Gruppieren / Gruppierung aufheben. Array: Reihe (Rechteckreihe, Kreisreihe).
- Align / Distribute / Dock: Ausrichten / Verteilen / Andocken. Flip / Mirror: spiegeln.
- Snap: einrasten. Grid: Gitter. Lock: sperren. Crop: zuschneiden. Mask: Maske.
- Variable text: Variabler Text. Selection: Auswahl. Page: Seite. Canvas: Zeichenfläche.
- Bed: Arbeitstisch. Work area: Arbeitsbereich. Panel: Bedienfeld. Toolbar: Werkzeugleiste.
- Ribbon: Menüband (tab Home: Start). Hotkeys: Tastenkürzel. Settings: Einstellungen.
- Theme: Design. Art library: Grafikbibliothek. Move (panel): Bewegen. Cut shapes: Formen zerschneiden.
- Cut table columns: Ebene, Modus, V/Leist, Ausg., Anz, Luft. The output column is "Ausg."
  (Ausgabe), never "Aus": that reads as "Off" above a tick box.

## French (fr)

"Vous" in help texts; commands as infinitives ("Décaler les formes"). A no-break space
(U+00A0) goes before : ; ? and ! and inside « », never a normal space: with a normal space the
punctuation or the closing » can wrap onto a line of its own. A test checks it (the checker
counts the colon at the end). Key names stay as in English: Ctrl, Shift,
Esc. Panel labels that are not translated yet are quoted « like this ».

- Job: tâche. Cut (laser): découpe, découper. Cut (clipboard): Couper.
- Layer: calque; sub-layer: sous-calque. Line / Fill / Image: Ligne / Remplissage / Image. Engrave: graver.
- Power: puissance. Speed: vitesse. Pass: passe. Interval: intervalle. Kerf: largeur de coupe.
- Overscan: surcourse. Air assist: assistance d'air. Frame: cadrer.
- Jog: déplacer. Home: prise d'origine. Origin: origine. Start point: point de départ.
- Cut direction: sens de découpe. Tab: attache. Focus test: Test de focale. Material test: Test de matériau.
- Cut library / Material library: Bibliothèque de découpe / de matériaux. Rotary: rotatif.
- Camera: caméra. Print and cut: Imprimer et découper. Device / Machine: appareil / machine.
- Console: console. Preview: Aperçu. Nudge: pas des flèches.
- Shape / Object: forme / objet. Path: tracé. Node: nœud. Weld: souder.
- Boolean union / difference / intersection: Union / Différence / Intersection booléenne.
- Offset: décalage (command: Décaler les formes). Trace: vectoriser. Bitmap: bitmap.
- Group / Ungroup: Grouper / Dissocier. Array: réseau (rectangulaire, circulaire, virtuel).
- Align / Distribute / Dock: Aligner / Répartir / Accoler. Flip: retourner. Mirror: symétrie.
- Snap: aimanter, aimantation. Grid: grille. Lock: verrouiller. Crop: rogner. Mask: masque.
- Variable text: Texte variable. Selection: sélection. Page: page. Canvas: zone de dessin.
- Bed: plateau. Work area: zone de travail. Panel: panneau. Toolbar: barre d'outils.
- Ribbon: ruban (tab Home: Accueil). Hotkeys: raccourcis. Settings and preferences: Paramètres.
- Take the key (from another command): Prendre la touche (not "reprendre", which means taking it back).
- Toolbar icon sizes agree with the plural « Icônes »: Petites, Moyennes, Grandes.
- Theme: thème. Art library: Bibliothèque graphique. Move (panel): Déplacement.
- Cut shapes: Diviser les formes. Feeder: chargeur. Camera control: Contrôle caméra.
- Cut table columns: Calque, Mode, Vit/Puis, Sor, Vis, Air.

## Italian (it)

Informal imperative in commands and help texts ("Salva", "Fai clic"). Key names stay as in
English: Ctrl, Shift, Esc. Panel labels that are not translated yet are quoted «like this».

- Job: lavoro. Cut (laser): taglio, tagliare. Cut (clipboard): Taglia.
- Layer: livello; sub-layer: sottolivello. Line / Fill / Image: Linea / Riempimento / Immagine. Engrave: incidere.
- Power: potenza. Speed: velocità. Pass: passata. Interval: intervallo. Kerf: larghezza di taglio.
- Overscan: oltrecorsa. Air assist: assistenza aria. Frame: contorno, percorrere il contorno.
- Jog: spostamento manuale. Home: azzeramento. Origin: origine. Start point: punto di partenza.
- Cut direction: direzione di taglio. Tab: ponticello. Focus test: Test di messa a fuoco.
- Material test: Test materiale. Cut library / Material library: Libreria di taglio / materiali.
- Rotary: rotativo. Camera: telecamera. Print and cut: Stampa e taglia.
- Device / Machine: dispositivo / macchina. Console: Console. Preview: Anteprima. Nudge: passo delle frecce.
- Shape / Object: forma / oggetto. Path: tracciato. Node: nodo. Weld: saldare.
- Boolean union / difference / intersection: Unione / Differenza / Intersezione booleana.
- Offset: Offset. Trace: vettorizzare. Dither: retinatura. Bitmap: bitmap.
- Group / Ungroup: Raggruppa / Separa. Array: serie (a griglia, circolare, virtuale).
- Align / Distribute / Dock: Allinea / Distribuisci / Accosta. Flip: capovolgi. Mirror: specchia.
- Snap: aggancio, aggancia. Grid: griglia. Lock: blocca. Crop: ritaglia. Mask: maschera.
- Variable text: Testo variabile. Selection: selezione. Page: pagina. Canvas: area di disegno.
- Bed: piano di lavoro. Work area: area di lavoro. Panel: pannello. Toolbar: barra degli strumenti.
- Ribbon: barra multifunzione (tab Home: Home). Hotkeys: scorciatoie. Settings: Impostazioni.
- Edit (menu, clipboard and undo): Modifica. Modify (ribbon tab, changing shapes): Trasforma.
- Toolbar icon sizes agree with the plural «Icone»: Piccole, Medie, Grandi.
- Theme: tema. Art library: Libreria grafica. Move (panel): Sposta. Cut shapes: Dividi forme.
- Feeder: caricatore. Camera control: Controllo telecamera.
- Cut table columns: Livello, Modalità, Vel/Pot, Usc, Vis, Aria.

## Spanish, Spain (es-ES)

Informal "tú" with the imperative in help texts ("Haz clic", "Pulsa"); commands use the
infinitive ("Guardar", "Abrir"). Key names stay as in English: Ctrl, Shift, Esc. Panel labels
that are not translated yet are quoted «like this». Spain words: ordenador, portátil, ratón,
pulsar, añadir, en directo.

- Job: trabajo. Cut (laser): corte, cortar. Cut (clipboard): Cortar.
- Layer: capa. Line / Fill / Image: Línea / Relleno / Imagen. Engrave: grabar.
- Power: potencia. Speed: velocidad. Pass: pasada. Interval: intervalo. Thickness: grosor.
- Air assist: asistencia de aire. Frame: recorrer el contorno.
- Home: referenciar. Origin: origen. Start point: punto de inicio.
- Cut direction: dirección de corte. Tab: puente. Focus test: Prueba de enfoque.
- Material test: Prueba de material. Cut library / Material library: Biblioteca de corte / de materiales.
- Rotary: rotativo. Camera: cámara. Print and cut: Imprimir y cortar.
- Device / Machine: dispositivo / máquina. Console: Consola. Preview: Vista previa. Nudge: paso de las flechas.
- Shape / Object: forma / objeto. Path: trazado. Node: nodo. Weld: soldar.
- Boolean union / difference / intersection: Unión / Diferencia / Intersección booleana.
- Offset: desfase, Desfasar formas. Trace: vectorizar. Bitmap: mapa de bits.
- Group / Ungroup: Agrupar / Desagrupar. Array: matriz.
- Align / Distribute / Dock: Alinear / Distribuir / Acoplar. Flip: voltear. Mirror: reflejar.
- Snap: ajustar a; the settings heading is "Ajuste magnético". Grid: cuadrícula. Lock: bloquear.
- Crop: recorte. Mask: máscara. Variable text: Texto variable. Selection: selección.
- Page: página. Canvas: lienzo. Bed: mesa. Work area: área de trabajo. Panel: panel.
- Toolbar: barra de herramientas. Ribbon tab Home: Inicio. Hotkeys: atajos.
- Settings (preferences and cut settings): Ajustes, ajustes de corte. Setup (ribbon group): Configuración.
- Edit (menu, clipboard and undo): Edición. Modify (ribbon tab, changing shapes): Modificar.
- Curve quality: Baja / Normal / Alta. Toolbar icon sizes agree with the plural «Iconos»: Pequeños, Medianos, Grandes.
- Theme: tema. Art library: Biblioteca de gráficos. Move (panel): Mover. Cut shapes: Dividir formas.
- Feeder: alimentador. Camera control: Control de cámara. Serial ports: Puertos serie.
- Cut table columns: Capa, Modo, Vel/Pot, Salida, Vis, Aire.

## Spanish, Latin America (es-LA)

Neutral Latin American Spanish, built from the Spain file and then changed wherever the two
regions say it differently; it is not a copy. Same "tú" imperative, quotes “like this”.
Everything not listed here is as in es-ES.

- Settings: Configuración (also "configuración de corte"). Setup (ribbon group): Preparación.
- Edit (menu and ribbon group): Editar. Modify (ribbon tab): Modificar.
- Computer: computadora. Laptop: laptop. Mouse: mouse. Press: presionar. Add: agregar.
- Live: en vivo. Thickness: espesor. Icons: Íconos. Serial ports: Puertos seriales.
- Same width: Igualar ancho. Paste in place: Pegar en el lugar. Autosave: Guardado automático.
- Rotate 90°: Girar 90° a la izquierda / derecha. Clear selection: Quitar selección.
- Take the key (hotkeys): Tomar la tecla.

## Portuguese, Brazil (pt)

Brazilian Portuguese: the project's research notes name no other variant, so "pt" is written
for Brazil. "Você" with the imperative in help texts ("Clique", "Pressione"); commands use the
infinitive. Key names stay as in English. Quotes “like this”.

- Job: trabalho. Cut (laser): corte, cortar. Cut (clipboard): Recortar.
- Layer: camada. Line / Fill / Image: Linha / Preenchimento / Imagem. Engrave: gravar.
- Power: potência. Speed: velocidade. Pass: passada. Interval: intervalo. Thickness: espessura.
- Air assist: assistência de ar. Frame: contornar.
- Home: referenciar. Origin: origem. Start point: ponto inicial.
- Cut direction: direção de corte. Tab: ponte. Focus test: Teste de foco.
- Material test: Teste de material. Cut library / Material library: Biblioteca de corte / de materiais.
- Rotary: rotativo. Camera: câmera. Print and cut: Imprimir e cortar.
- Device / Machine: dispositivo / máquina. Console: Console. Preview: Pré-visualização. Nudge: passo das setas.
- Shape / Object: forma / objeto. Path: caminho. Node: nó. Weld: soldar.
- Boolean union / difference / intersection: União / Diferença / Interseção booleana.
- Offset: Offset. Trace: vetorizar. Bitmap: bitmap.
- Group / Ungroup: Agrupar / Desagrupar. Array: matriz.
- Align / Distribute / Dock: Alinhar / Distribuir / Encostar. Flip: inverter. Mirror: espelhar.
- Snap: ajustar a; the settings heading is "Ajuste magnético". Grid: grade. Lock: bloquear.
- Crop: recorte. Mask: máscara. Variable text: Texto variável. Selection: seleção.
- Page: página. Canvas: área de desenho. Bed: mesa. Work area: área de trabalho. Panel: painel.
- Toolbar: barra de ferramentas. Ribbon tab Home: Início. Hotkeys: atalhos. Settings: Configurações.
- Edit (menu, clipboard and undo): Editar. Modify (ribbon tab, changing shapes): Modificar.
- Curve quality: Baixa / Normal / Alta. Toolbar icon sizes agree with the plural «Ícones»: Pequenos, Médios, Grandes.
- Theme: tema. Art library: Biblioteca de artes. Move (panel): Mover. Cut shapes: Dividir formas.
- Feeder: alimentador. Camera control: Controle da câmera. Serial ports: Portas seriais.
- Computer: computador. Laptop: notebook. Screen: tela.
- Cut table columns: Camada, Modo, Vel/Pot, Saída, Vis, Ar.
