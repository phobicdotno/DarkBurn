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
- Width / Height: Ancho / Alto, also in "Igualar ancho" and "Igualar alto". The machine's height
  follower is a part name of its own: seguidor de altura.
- Reset: Restablecer. Backup (a saved copy of a file): copia de seguridad. Backup path (the
  outline an image keeps after a crop): trazado de respaldo.
- Start (the laser panel button, not translated yet): «Start», as in "Ignorar «Start» …".

## Spanish, Latin America (es-LA)

Neutral Latin American Spanish, built from the Spain file and then changed wherever the two
regions say it differently; it is not a copy. Same "tú" imperative, quotes “like this”.
Everything not listed here is as in es-ES.

- Settings: Configuración (also "configuración de corte"). Setup (ribbon group): Preparación.
- Edit (menu and ribbon group): Editar. Modify (ribbon tab): Modificar.
- Computer: computadora. Laptop: laptop. Mouse: mouse. Press: presionar. Add: agregar.
- Live: en vivo. Thickness: espesor. Icons: Íconos. Serial ports: Puertos seriales.
- Backup (a saved copy of a file): respaldo, as in the backup path. Paste in place: Pegar en el lugar. Autosave: Guardado automático.
- Rotate 90°: Girar 90° a la izquierda / derecha. Clear selection: Quitar selección.
- Take the key (hotkeys): Tomar la tecla. Start (laser panel button): “Start”.

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
- Width / Height: Largura / Altura. Reset: Restaurar. Backup: backup, also in the image's
  "caminho de backup".
- Start (the laser panel button, not translated yet): “Start”.

## Dutch (nl)

Informal "je" with the imperative in help texts ("Klik", "Druk op"); commands use the
infinitive ("Opslaan", "Openen"). Key names stay as in English. Panel labels that are not
translated yet are quoted “like this”.

- Job: taak. Cut (laser): snijden, as in Snij-instellingen, Snijbibliotheek, snijrichting, Snijpalet.
  Cut (clipboard): Knippen. Cut shapes: Vormen splitsen.
- Layer: laag. Line / Fill / Image: Lijn / Vullen / Afbeelding. Engrave: graveren.
- Power: vermogen. Speed: snelheid. Pass: doorgang. Interval: interval. Thickness: dikte.
- Air assist: luchtondersteuning. Frame: de omtrek aflopen.
- Home: thuispositie. Origin: nulpunt. Start point: beginpunt.
- Tab: brug, bruggen. Rotary: rotatie-as. Camera: camera. Print and cut: Printen en snijden.
- Device / Machine: apparaat / machine. Command: opdracht. Preview: Voorbeeld. Nudge: stap (met pijltoetsen).
- Shape / Object: vorm / object. Path: pad. Node: knooppunt. Weld: samensmelten.
- Boolean union / difference / intersection: Booleaanse vereniging / Booleaans verschil / Booleaanse doorsnede.
- Offset: Offset, "Vormen offsetten". Trace: overtrekken. Bitmap: bitmap.
- Group / Ungroup: Groeperen / Groepering opheffen. Array: reeks (Rasterreeks, Cirkelreeks, Virtuele reeks).
- Align / Distribute / Dock: Uitlijnen / Verdelen / Aanschuiven. Flip and Mirror: spiegelen.
- Snap: kleven. Grid: raster. Lock: vergrendelen. Crop: bijsnijden. Pan: pannen.
- Selection: selectie. Page: pagina. Canvas: tekenvlak. Bed: werkbed. Work area: werkgebied.
- Toolbar: werkbalk. Ribbon tab Home: Start. Hotkeys: sneltoetsen. Settings: Instellingen.
  Preferences (the settings file): voorkeuren.
- Edit (menu, clipboard and undo): Bewerken. Modify (ribbon tab): Wijzigen. Arrange: Schikken.
- Art library: Beeldbank. Library entry and art item: item. Move (panel): Verplaatsen.
- Feeder: invoer. Camera control: Camerabediening. Frame Selection (view): Zoomen naar selectie.
- Cut table columns: Laag, Modus, Snelh/Verm, Uitv., Zicht, Lucht ("Uit" would read as Off).
- Width / Height: Breedte / Hoogte. Reset: herstellen. Backup (a saved copy of a file): back-up.
  Backup path (the outline an image keeps after a crop): reservepad.
- Start (the laser panel button, not translated yet): “Start”.

## Danish (da)

"Du" with the imperative in help texts and in commands ("Gem", "Åbn", "Klip", "Sæt ind").
Key names stay as in English. Panel labels that are not translated yet are quoted »like this«.

- Job: job. Cut (laser): skæring, skære-, as in Skæreindstillinger, Skærebibliotek, skæreretning,
  Skærepalet. Cut (clipboard): Klip. Cut shapes: Opdel figurer.
- Layer: lag. Line / Fill / Image: Linje / Udfyldning / Billede.
- Power: effekt. Speed: hastighed. Pass: gennemløb. Thickness: tykkelse.
- Air assist: luftassistance. Frame: køre omridset.
- Home: hjem. Origin: nulpunkt. Start point: startpunkt.
- Tab: tap, tappe. Rotary: rotationsakse. Print and cut: Print og skær.
- Device / Machine: enhed / maskine. Command: kommando. Preview: Eksempel. Nudge: flyt med piletaster.
- Shape / Object: figur / objekt. Selection: markering. Path: sti. Node: knudepunkt. Weld: svejse.
- Boolean union / difference / intersection: Boolesk forening / forskel / fællesmængde.
- Offset: forskyde, Forskydning. Trace: vektorisere. Bitmap: bitmap.
- Group / Ungroup: Gruppér / Opløs gruppe. Array: mønster.
- Align / Distribute / Dock: Juster / Fordel / Skub. Flip: spejlvende. Mirror: spejle.
  Align top / bottom / centres: Juster øverst / nederst / midt. Assign to layer: Tildel til lag.
- Snap: fastgøre, Fastgøring. Grid: gitter. Lock: låse. Crop: beskæring. Pan: panorere.
- Page: side. Canvas: lærred. Bed: arbejdsbord. Work area: arbejdsområde.
- Toolbar: værktøjslinje. Ribbon tab Home: Start. Hotkeys: genvejstaster. Settings: Indstillinger.
  Preferences (the settings file): præferencer.
- Edit: Rediger. Modify (ribbon tab): Ændr. Arrange: Arranger. File menu: Filer.
- Mode: tilstand, also in the cut table. Art library: Grafikbibliotek. Library entry: post. Art item: element.
- Feeder: fremføring. Camera control: Kamerastyring.
- Cut table columns: Lag, Tilstand, Hast/Eff, Ud, Vis, Luft.
- Width / Height: Bredde / Højde. Reset: Nulstil. Backup (a saved copy of a file): sikkerhedskopi.
  Backup path (the outline an image keeps after a crop): reservesti.
- Start (the laser panel button, not translated yet): »Start«.

## Swedish (sv)

"Du" with the imperative in help texts and in commands ("Spara", "Öppna", "Klipp ut").
Key names stay as in English. Panel labels that are not translated yet are quoted ”like this”.

- Job: jobb. Cut (laser): skärning, skär-, as in Skärinställningar, Skärbibliotek, skärriktning,
  Skärpalett. Cut (clipboard): Klipp ut. Cut shapes: Dela former.
- Layer: lager. Line / Fill / Image: Linje / Fyllning / Bild.
- Power: effekt. Speed: hastighet. Pass: pass. Thickness: tjocklek.
- Air assist: luftassistans. Frame: köra konturen.
- Home: hemläge. Origin: nollpunkt. Start point: startpunkt.
- Tab: brygga, bryggor. Rotary: rotationsaxel. Print and cut: Utskrift och skärning.
- Device / Machine / Controller: enhet / maskin / styrenhet. Command: kommando. Preview: Förhandsvisning.
- Shape / Object: form / objekt. Selection: markering. Path: bana. Node: nod. Weld: svetsa.
- Boolean union / difference / intersection: Boolesk union / differens / Booleskt snitt.
- Offset: förskjuta, Förskjutning. Trace: vektorisera. Bitmap: bitmapp. Output: utmatning.
- Group / Ungroup: Gruppera / Dela upp grupp. Array: mönster.
- Align / Distribute / Dock: Justera / Fördela / Docka. Flip: vända. Mirror: spegla.
- Snap: fästa, Fästning. Grid: rutnät. Lock: låsa. Crop: beskärning. Pan: panorera.
- Page: sida. Canvas: arbetsyta. Bed: arbetsbädd. Work area: arbetsområde.
- Toolbar: verktygsfält. Ribbon tab Home: Start. Hotkeys: kortkommandon. Settings: Inställningar.
  Preferences (the settings file): preferenser.
- Edit: Redigera. Modify (ribbon tab): Ändra. Arrange: Ordna. File menu: Arkiv.
- Mode: läge. Art library: Bildbibliotek. Library entry: post. Art item: objekt.
- Feeder: matning. Camera control: Kamerastyrning.
- Cut table columns: Lager, Läge, Hast/Eff, Ut, Syn, Luft.
- Width / Height: Bredd / Höjd. Reset: Återställ. Backup (a saved copy of a file): säkerhetskopia.
  Backup path (the outline an image keeps after a crop): reservbana.
- Start (the laser panel button, not translated yet): ”Start”.

## Finnish (fi)

Imperative in commands and help texts ("Tallenna", "Avaa", "Napsauta").
Key names stay as in English. Panel labels that are not translated yet are quoted ”like this”.

- Job: työ. Cut (laser): leikkaus, as in Leikkausasetukset, leikkauskirjasto, leikkaussuunta,
  Leikkauspaletti. Cut (clipboard): Leikkaa. Cut shapes: Jaa muodot.
- Layer: taso. Line / Fill / Image: Viiva / Täyttö / Kuva.
- Power: teho. Speed: nopeus. Pass: kierros. Interval: riviväli. Thickness: paksuus.
- Air assist: ilmapuhallus. Frame: ajaa ääriviiva.
- Home: kotiajo. Origin: nollapiste. Start point: aloituspiste.
- Tab: silta, sillat. Rotary: pyöröakseli. Print and cut: Tulosta ja leikkaa.
- Device / Machine / Controller: laite / kone / ohjain. Command: komento. Preview: Esikatselu.
- Shape / Object: muoto / objekti. Selection: valinta. Path: polku. Node: solmu. Weld: hitsata.
- Boolean union / difference / intersection: Boolen yhdiste / erotus / leikkaus.
- Offset: siirtymä. Trace: vektoroida. Bitmap: bittikartta.
- Group / Ungroup: Ryhmitä / Pura ryhmitys. Array: sarja (Ruudukkosarja, Ympyräsarja, Virtuaalisarja).
- Align / Distribute / Dock: Tasaa / Levitä / Työnnä (push a shape until it touches; the
  ribbon group and toolbar: Työntö). Not "telakoida", which is docking a window or panel. Flip: kääntää. Mirror: peilata.
- Snap: kiinnitys. Grid: ruudukko. Lock: lukita. Crop: rajaus. Pan: panoroida.
- Page: sivu. Canvas: piirtoalue. Work area: työalue.
- Toolbar: työkalurivi. Ribbon tab Home: Aloitus. Hotkeys: pikanäppäimet. Settings and
  preferences: asetukset.
- Edit: Muokkaa. Modify (ribbon tab): Muunna. Arrange: Järjestä. File menu: Tiedosto.
- Mode: tila. Art library: Kuvakirjasto. Library entry: merkintä. Art item: kohde.
- Feeder: syötin. Camera control: Kameran ohjaus.
- Output (a layer or job sent to the laser): ajo, as in Ajon käyrätoleranssi. The cut table
  column is headed Ulos; the help names it "ajo (Ulos)". Never "tulostus", which is Print.
- Cut table columns: Taso, Tila, Nop/Teho, Ulos, Näk, Ilma.
- Width / Height: Leveys / Korkeus. Reset: Nollaa. Backup (a saved copy of a file): varmuuskopio.
  Backup path (the outline an image keeps after a crop): varapolku.
- Start (the laser panel button, not translated yet): ”Start”.

## Polish (pl)

Imperative in commands and help texts ("Zapisz", "Otwórz", "Kliknij"), the informal "ty" in
help texts. Key names stay as in English. Panel labels that are not translated yet are
quoted „like this”. Polish words change their endings, so the one-word check compares stems
("kopi zapasow" finds "kopia zapasowa" and "kopię zapasową").

- Job: zadanie. Cut (laser): cięcie, as in Ustawienia cięcia, biblioteka cięcia, kierunek cięcia,
  Paleta cięcia. Cut (clipboard): Wytnij. Cut shapes: Podziel kształty.
- Layer: warstwa. Line / Fill / Image: Linia / Wypełnienie / Obraz.
- Power: moc. Speed: prędkość. Pass: przejście. Interval: odstęp linii. Thickness: grubość.
- Air assist: nadmuch. Frame: obrysować.
- Home: bazowanie. Origin: punkt zerowy. Start point: punkt początkowy.
- Tab: mostek, mostki. Rotary: oś obrotowa. Print and cut: Drukuj i tnij.
- Device / Machine / Controller: urządzenie / maszyna / kontroler. Command: polecenie. Preview: Podgląd.
- Shape / Object: kształt / obiekt. Selection: zaznaczenie (Zaznacz). Path: ścieżka. Node: węzeł.
  Weld: spawać.
- Boolean union / difference / intersection: Suma logiczna / Różnica logiczna / Część wspólna.
- Offset: odsunięcie. Trace: wektoryzować. Bitmap: mapa bitowa.
- Group / Ungroup: Grupuj / Rozgrupuj. Array: szyk (prostokątny, kołowy, wirtualny).
- Align / Distribute / Dock: Wyrównaj / Rozłóż / Dosuń. Flip: przerzucić. Mirror: odbić lustrzanie.
- Snap: przyciąganie. Grid: siatka. Lock: zablokować. Crop: przycięcie. Pan: przesunąć widok.
- Page: strona. Canvas: płótno. Work area: obszar roboczy.
- Toolbar: pasek narzędzi. Ribbon tab Home: Start. Hotkeys: skróty klawiszowe.
  Settings: Ustawienia. Preferences (the settings file): preferencje.
- Edit: Edycja. Modify (ribbon tab): Modyfikuj. Arrange: Rozmieść. File menu: Plik.
- Mode: tryb. Art library: Biblioteka grafik. Library entry: wpis (masculine, so a linked
  entry is "połączony"). Art item: element.
- Feeder: podajnik. Camera control: Sterowanie kamerą.
- Cut table columns: Warstwa, Tryb, Pręd/Moc, Wyj, Wid, Nadm.
- Status bar count: szt. (one word for object and objects).
- Width / Height: Szerokość / Wysokość. Reset: Resetuj. Backup (a saved copy of a file):
  kopia zapasowa. Backup path (the outline an image keeps after a crop): ścieżka zapasowa.
- Start (the laser panel button, not translated yet): „Start”.

## Czech (cs)

Infinitive in commands ("Uložit", "Otevřít", "Vložit"), the formal imperative in help texts
("Klepněte", "Stiskněte"). Key names stay as in English. Panel labels that are not translated
yet are quoted „like this“. Czech words change their endings, so the one-word check compares
stems ("záloh" finds "záloha" and "zálohu").

- Job: úloha. Cut (laser): řez, řezání, as in Nastavení řezu, knihovna řezání, směr řezu,
  Paleta řezů. Cut (clipboard): Vyjmout. Cut shapes: Rozdělit tvary.
- Layer: vrstva. Line / Fill / Image: Čára / Výplň / Obrázek.
- Power: výkon. Speed: rychlost. Pass: průchod. Interval: rozteč řádků. Thickness: tloušťka.
- Air assist: ofuk. Frame: objet obrys.
- Home: výchozí poloha. Origin: počátek. Start point: počáteční bod.
- Tab: můstek, můstky. Rotary: rotační osa. Print and cut: Tisk a řez.
- Device / Machine / Controller: zařízení / stroj / řadič. Command: příkaz. Preview: Náhled.
- Shape / Object: tvar / objekt. Selection: výběr (Vybrat). Path: cesta. Node: uzel. Weld: svařit.
- Boolean union / difference / intersection: Booleovské sjednocení / Booleovský rozdíl / Booleovský průnik.
- Offset: odsazení. Trace: vektorizovat. Bitmap: bitmapa. Break apart: Rozložit.
- Group / Ungroup: Seskupit / Zrušit seskupení. Array: pole (pravoúhlé, kruhové, virtuální).
- Align / Distribute / Dock: Zarovnat / Rozmístit / Přisunout. Nest (pack shapes onto the
  sheet): Naskládat, never "vnořit", which means put one thing inside another. Flip: překlopit. Mirror: zrcadlit.
- Snap: přichytávání. Grid: mřížka. Lock: zamknout. Crop: oříznutí. Pan: posunout zobrazení.
- Page: stránka. Canvas: plátno. Work area: pracovní plocha.
- Toolbar: panel nástrojů. Ribbon tab Home: Domů. Ribbon group Setup: Příprava.
  Hotkeys: klávesové zkratky. Settings: Nastavení. Preferences (the settings file): předvolby.
  Presets: přednastavení.
- Edit: Úpravy. Modify (ribbon tab): Upravit. Arrange: Uspořádat. File menu: Soubor.
- Mode: režim. Art library: Knihovna grafiky. Library entry and art item: položka.
- Feeder: podavač. Camera control: Ovládání kamery.
- Cut table columns: Vrstva, Režim, Rych/Výk, Výst, Zobr, Ofuk.
- Status bar count: ks (one word for object and objects).
- Hotkey conflicts name the key first, so the key is the subject: "je už přiřazena příkazu",
  "přiřazena více příkazům".
- Width / Height: Šířka / Výška. Reset: Resetovat. Backup (a saved copy of a file): záloha.
  Backup path (the outline an image keeps after a crop): záložní cesta.
- Start (the laser panel button, not translated yet): „Start“.

## Hungarian (hu)

Commands are nouns ("Mentés", "Megnyitás", "Beillesztés"), as Hungarian programs name them; help
texts use the formal imperative ("Kattintson", "Nyomja meg"). Key names stay as in English, with
the ending joined by a hyphen ("Ctrl-lal"). Panel labels that are not translated yet are quoted
„like this”. Hungarian adds endings to its words, so the one-word check compares stems
("biztonsági mentés" finds "biztonsági mentésből").

- Job: munka. Cut (laser): vágás, as in Vágási beállítások, vágási könyvtár, vágási irány,
  Vágási paletta. Cut (clipboard): Kivágás. Cut shapes: Alakzatok darabolása.
- Layer: réteg. Line / Fill / Image: Vonal / Kitöltés / Kép.
- Power: teljesítmény. Speed: sebesség. Pass: menet. Interval: sorköz. Thickness: vastagság.
- Air assist: légfúvás. Frame: körbejárás.
- Home: alaphelyzet. Origin: nullpont. Start point: kezdőpont.
- Tab: híd, hidak. Rotary: forgótengely. Print and cut: Nyomtatás és vágás; its targets: illesztőjel.
- Device / Machine / Controller: eszköz / gép / vezérlő. Command: parancs. Preview: Előnézet.
- Shape / Object: alakzat / objektum. Selection: kijelölés. Path: útvonal. Node: csomópont.
  Weld: összeolvasztás.
- Boolean union / difference / intersection: Logikai egyesítés / különbség / metszet.
- Offset: eltolás. Trace: vektorizálás. Bitmap: bitkép. Break apart: Szétbontás.
- Group / Ungroup: Csoportosítás / Csoportbontás. Array: tömb (Rácstömb, Körkörös tömb, Virtuális tömb).
- Align / Distribute / Dock: Igazítás / Elosztás / Ütköztetés. Flip: átfordítás. Mirror: tükrözés.
- Snap: illesztés. Grid: rács. Lock: zárolás. Crop: körülvágás. Mask: maszk. Pan: nézet mozgatása.
- Page: lap. Canvas: rajzterület. Work area: munkaterület.
- Toolbar: eszköztár. Ribbon tab Home: Kezdőlap. Ribbon group Setup: Előkészítés.
  Hotkeys: gyorsbillentyűk. Settings and preferences: beállítások.
- Edit: Szerkesztés. Modify (ribbon tab): Módosítás. Arrange: Elrendezés. File menu: Fájl.
- Mode: mód. Library: könyvtár; Art library: Grafikakönyvtár; Material library: Anyagkönyvtár.
  Library entry: bejegyzés. Art item: elem. Link / Unlink: Csatolás / Leválasztás.
- Feeder: adagoló. Camera control: Kameravezérlés. Output: kimenet.
- Cut table columns: Réteg, Mód, Seb/Telj, Kim., Lát, Lég. Output is "Kim.", never "Ki", which reads as Off.
- Status bar count: objektum (one word for object and objects).
- Width / Height: Szélesség / Magasság. Reset: visszaállítás. Backup (a saved copy of a file):
  biztonsági mentés. Backup path (the outline an image keeps after a crop): tartalék útvonal.
- Start (the laser panel button, not translated yet): „Start”.
- Hotkey conflicts: the key is the subject and the command owns it ("Ctrl+S már foglalt, ez a
  parancs használja ..."); a bare "használja" reads as "use it".

## Slovenian (sl)

The imperative in commands ("Shrani", "Odpri", "Prilepi") and the formal imperative in help texts
("Kliknite", "Pritisnite"). Key names stay as in English. Panel labels that are not translated
yet are quoted »like this«. Slovenian words change their endings, so the one-word check compares
stems ("varnostn kopij" finds "varnostna kopija" and "varnostne kopije").

- Job: opravilo. Cut (laser): rez, rezanje, as in Nastavitve rezanja, knjižnica rezanja, smer reza,
  Paleta rezov. Cut (clipboard): Izreži. Cut shapes: Razdeli oblike.
- Layer: plast. Line / Fill / Image: Črta / Polnilo / Slika.
- Power: moč. Speed: hitrost. Pass: prehod. Interval: razmik vrstic. Thickness: debelina.
- Air assist: pihanje zraka. Frame: obhod obrisa.
- Home: referenčna lega. Origin: izhodišče. Start point: začetna točka.
- Tab: mostiček, mostički. Rotary: rotacijska os. Print and cut: Tiskanje in rezanje; its targets: oznake.
- Device / Machine / Controller: naprava / stroj / krmilnik. Command: ukaz. Preview: Predogled.
- Shape / Object: oblika / objekt. Selection: izbor. Path: pot. Node: vozlišče. Weld: zvari.
- Boolean union / difference / intersection: Logična unija / razlika / Logični presek.
- Offset: odmik (Odmakni oblike). Trace: vektoriziraj. Bitmap: bitna slika. Break apart: Razbij.
- Group / Ungroup: Združi / Razdruži. Array: polje (Mrežno, Krožno, Navidezno polje).
- Align / Distribute / Dock: Poravnaj / Razporedi / Prisloni. Flip: obrni. Mirror: zrcali.
- Snap: pripenjanje. Grid: mreža. Lock: zakleni. Crop: obrezovanje. Mask: maska. Pan: premik pogleda.
- Page: stran. Canvas: platno. Work area: delovno območje.
- Toolbar: orodna vrstica. Ribbon tab Home: Osnovno. Ribbon group Setup: Priprava.
  Hotkeys: bližnjice. Settings and preferences: nastavitve.
- Edit: Urejanje. Modify (ribbon tab): Spremeni. Arrange: Postavitev. File menu: Datoteka.
- Mode: način. Art library: Knjižnica grafik. Library entry: vnos (masculine: "povezan").
  Art item: element. Link / Unlink: Poveži / Prekini povezavo.
- Feeder: podajalnik. Camera control: Upravljanje kamere. Output: izhod; Print: tisk.
- Cut table columns: Plast, Način, Hit/Moč, Izh, Vid, Zrak.
- Status bar count: obj. (one word for object and objects).
- Width / Height: Širina / Višina. Reset: Ponastavi. Backup (a saved copy of a file):
  varnostna kopija. Backup path (the outline an image keeps after a crop): rezervna pot.
- Start (the laser panel button, not translated yet): »Start«.
- Hotkey conflicts: the key (tipka, feminine) is the subject: "Ctrl+S je že dodeljena ukazu",
  "dodeljena več ukazom"; "uporablja" would make the key use the command.
- Choices agree with their label: Toolbar icons (Ikone) are Majhne / Srednje / Velike; Curve
  quality (kakovost) is Groba / Običajna / Fina.

## Turkish (tr)

The imperative in commands ("Kaydet", "Aç", "Yapıştır") and the formal imperative in help texts
("Tıklayın", "Basın"). Key names stay as in English. Panel labels that are not translated yet are
quoted “like this”. Turkish adds suffixes to its words, so the one-word check compares stems
("genişlik" finds "Genişlikleri"); forms that change the last letter ("yedeği") are avoided.

- Job: iş. Cut (laser): kesim, as in Kesim ayarları, kesim kütüphanesi, kesim yönü, Kesim paleti.
  Cut (clipboard): Kes. Cut shapes: Şekilleri böl.
- Layer: katman. Line / Fill / Image: Çizgi / Dolgu / Görüntü.
- Power: güç. Speed: hız. Pass: geçiş. Interval: aralık. Thickness: kalınlık.
- Air assist: hava desteği. Frame: çerçeveleme.
- Home: referans. Origin: orijin. Start point: başlangıç noktası.
- Tab: köprü. Rotary: döner eksen. Print and cut: Yazdır ve kes; its targets: hedef işaretleri.
- Device / Machine / Controller: cihaz / makine / denetleyici. Command: komut. Preview: Önizleme.
- Shape / Object: şekil / nesne. Selection: seçim. Path: yol. Node: düğüm. Weld: kaynakla.
- Boolean union / difference / intersection: Boole birleşimi / farkı / kesişimi.
- Offset: ofset. Trace: vektörleştir. Bitmap: bit eşlem. Break apart: Parçala.
- Group / Ungroup: Grupla / Grubu çöz. Array: dizi (Izgara dizisi, Dairesel dizi, Sanal dizi).
- Align / Distribute / Dock: Hizala / Dağıt / Yasla. Nest: iç içe yerleştir. Flip: çevir.
  Mirror: aynala.
- Snap: kenetleme, never "yapıştır", which is Paste. Grid: ızgara. Lock: kilitle. Crop: kırpma.
  Mask: maske. Pan: kaydır.
- Page: sayfa. Canvas: tuval. Work area: çalışma alanı.
- Toolbar: araç çubuğu. Ribbon tab Home: Giriş. Ribbon group Setup: Hazırlık.
  Hotkeys: kısayol tuşları. Settings: Ayarlar. Preferences (the settings file): tercihler.
- Edit: Düzen. Modify (ribbon tab): Değiştir. Arrange: Yerleştir. File menu: Dosya.
- Mode: mod. Library: kütüphane; Art library: Grafik kütüphanesi. Library entry: kayıt.
  Art item: öğe. Link / Unlink: Bağla / Bağlantıyı kes.
- Feeder: besleyici. Camera control: Kamera kontrolü. Output: çıkış; Print: yazdır.
- Cut table columns: Katman, Mod, Hız/Güç, Çıkış, Gör, Hava.
- Status bar count: nesne (one word for object and objects).
- Width / Height: Genişlik / Yükseklik. Reset: Sıfırla. Backup (a saved copy of a file): yedek.
  Backup path (the outline an image keeps after a crop): yedek yol.
- Start (the laser panel button, not translated yet): “Start”.
- Flatten (a virtual array into plain shapes): düzleştir ("Diziyi düzleştir"); "dağıt" is only
  Distribute.
- Captions in capitals keep the Turkish i: "Birimler" is shown as "BİRİMLER". The program does
  this itself, so the file holds normal case.

## Greek (el)

Commands are nouns, as Greek programs write them ("Αποθήκευση", "Άνοιγμα", "Επικόλληση"); help
texts use the formal imperative ("Κάντε κλικ", "Πατήστε"). The Greek question mark is ";", never
"?", and there is no ano teleia (·): a comma or a full stop does. Panel labels that are not
translated yet are quoted «like this». Greek changes the endings of its words, so the one-word
check compares stems ("επαναφορ" finds "Επαναφορά" and "επαναφοράς").

- Job: εργασία. Cut (laser): κοπή, as in Ρυθμίσεις κοπής, βιβλιοθήκη κοπής, κατεύθυνση κοπής,
  Παλέτα κοπής. Cut (clipboard): Αποκοπή. Cut shapes: Διαχωρισμός σχημάτων.
- Layer: επίπεδο. Line / Fill / Image: Γραμμή / Γέμισμα / Εικόνα.
- Power: ισχύς. Speed: ταχύτητα. Pass: πέρασμα. Interval: διάστημα. Thickness: πάχος.
- Air assist: φύσημα αέρα (column: Αέρας). Frame (the laser traces the outline): πλαισίωση.
  Frame Selection (the view): Προσαρμογή στην επιλογή.
- Home: αρχική θέση. Origin: αφετηρία. Start point: σημείο έναρξης.
- Tab: γέφυρα. Rotary: περιστροφικός άξονας. Print and cut: Εκτύπωση και κοπή; its targets:
  σημάδια.
- Device / Machine / Controller: συσκευή / μηχάνημα / ελεγκτής. Command: εντολή.
  Preview: Προεπισκόπηση.
- Shape / Object: σχήμα / αντικείμενο. Selection: επιλογή. Path: διαδρομή. Node: κόμβος.
  Weld: Συγκόλληση.
- Boolean union / difference / intersection: Λογική ένωση / διαφορά / τομή; the group: Λογικές
  πράξεις.
- Offset: Μετατόπιση (Move is μετακίνηση). Trace: Ανίχνευση. Bitmap: ψηφιογραφία.
- Group / Ungroup: Ομαδοποίηση / Κατάργηση ομάδας. Break apart: Διάσπαση. Auto-join: συνένωση.
- Array: συστοιχία (Συστοιχία πλέγματος, Κυκλική συστοιχία, Εικονική συστοιχία).
- Align: Στοίχιση; the ribbon group Alignment: Ευθυγράμμιση. Distribute: Κατανομή.
  Dock: Ώθηση. Nest: Ένθεση. Flip: Αναστροφή. Mirror: Κατοπτρισμός.
- Snap: προσκόλληση. Grid: πλέγμα. Lock: κλείδωμα. Crop: περικοπή. Mask: μάσκα.
  Pan: Μετακίνηση προβολής. Nudge (arrow keys): μικρομετακίνηση.
- Page: σελίδα. Canvas: καμβάς. Work area: περιοχή εργασίας. Bed: τραπέζι. Panel: πάνελ.
  Window layout: διάταξη.
- Toolbar: γραμμή εργαλείων. Ribbon tab Home: Κεντρική. Ribbon group Setup: Προετοιμασία.
  Hotkeys: συντομεύσεις. Settings: Ρυθμίσεις. Preferences (the settings file): προτιμήσεις.
- Edit: Επεξεργασία. Modify (ribbon tab): Τροποποίηση. Arrange: Τακτοποίηση. File menu: Αρχείο.
- Mode: λειτουργία. Art library: Βιβλιοθήκη γραφικών. Library entry: καταχώριση (feminine:
  "συνδεδεμένη"). Art item: στοιχείο. Link / Unlink: Σύνδεση / Αποσύνδεση.
- Feeder: τροφοδότης. Camera control: Έλεγχος κάμερας. Print: εκτύπωση.
- Output and Exit are both Έξοδος, as in other Greek programs; they never stand side by side.
- Cut table columns: Επίπεδο, Λειτ., Ταχ/Ισχ, Έξοδος, Ορατό, Αέρας.
- Status bar count: αντικείμενο / αντικείμενα.
- Width / Height: Πλάτος / Ύψος. Reset: Επαναφορά. Backup (a saved copy of a file): αντίγραφο
  ασφαλείας. Backup path (the outline an image keeps after a crop): εφεδρική διαδρομή.
- Debug log: καταγραφή αποσφαλμάτωσης (debugging, not a log of errors only).
- Start (the laser panel button, not translated yet): «Start».
- Hotkey conflicts: "Ctrl+S χρησιμοποιείται ήδη από την εντολή ..."; Take the key: Μεταφορά
  πλήκτρου.
- Choices agree with their label: Toolbar icons (εικονίδια) are Μικρά / Μεσαία / Μεγάλα; Curve
  quality (ποιότητα) is Χαμηλή / Κανονική / Υψηλή; Theme (θέμα) is Σκοτεινό / Φωτεινό.
- Captions in capitals drop the accent: "Μονάδες" is shown as "ΜΟΝΑΔΕΣ", and "τσάι" as "ΤΣΑΪ".
  The program does this itself, so the file holds normal case. The hotkey search ignores accents.

## Russian (ru)

The infinitive in commands ("Сохранить", "Открыть", "Вставить") and the formal imperative in
help texts ("Щелкните", "Нажмите"). The letter ё is written е, as most programs do; the hotkey
search treats them as one letter. Panel labels that are not translated yet are quoted «like
this». Russian changes the endings of its words, so the one-word check compares stems
("резервн копи" finds "резервную копию").

- Job: задание. Cut (laser): резка, as in Параметры резки, библиотека резки, направление резки,
  Палитра резки. Cut (clipboard): Вырезать. Cut shapes: Разрезать фигуры.
- Layer: слой. Line / Fill / Image: Линия / Заливка / Изображение.
- Power: мощность. Speed: скорость. Pass: проход. Interval: интервал. Thickness: толщина.
- Air assist: обдув. Frame: рамка.
- Home: исходная позиция. Origin: начало координат. Start point: начальная точка.
- Tab: перемычка. Rotary: поворотная ось. Print and cut: Печать и резка; its targets: метки.
- Device / Machine / Controller: устройство / станок / контроллер. Command: команда.
  Preview: Предпросмотр.
- Shape / Object: фигура / объект. Selection: выделение. Path: контур. Node: узел.
  Weld: Слить фигуры.
- Boolean union / difference / intersection: Логическое объединение / вычитание / пересечение;
  the group: Логические операции.
- Offset: Смещение контура (Move is Перемещение). Trace: Векторизация. Bitmap: растр.
- Group / Ungroup: Сгруппировать / Разгруппировать. Break apart: Разъединить.
- Array: массив (Массив сеткой, Круговой массив, Виртуальный массив).
- Align / Distribute / Dock: Выровнять / Распределить / Прижать. Nest: Раскладка.
  Flip: Отразить (the group: Отражение). Mirror: Зеркалировать.
- Snap: привязка. Grid: сетка. Lock: заблокировать. Crop: обрезка. Mask: маска.
  Pan: Панорамирование. Nudge (arrow keys): сдвиг.
- Page: страница. Canvas: холст. Work area: рабочая область. Bed: рабочий стол.
- Toolbar: панель инструментов. Ribbon tab Home: Главная. Ribbon group Setup: Настройка.
  Hotkeys: горячие клавиши. Settings and preferences: настройки. Cut settings: Параметры резки.
- Edit: Правка. Modify (ribbon tab): Изменить. Arrange: Упорядочить. File menu: Файл.
- Mode: режим. Art library: Библиотека графики. Library entry: запись (feminine: "связана").
  Art item: элемент. Link / Unlink: Связать / Отвязать.
- Feeder: податчик. Camera control: Управление камерой. Output: вывод; Print: печать.
- Cut table columns: Слой, Режим, Скор/Мощн, Вывод, Показ, Обдув
  ("Видим" alone would read as "we see"). Off would be Выкл.
- Status bar count: шт. (one word for object and objects).
- Width / Height: Ширина / Высота. Reset: Сбросить. Backup (a saved copy of a file): резервная
  копия. Backup path (the outline an image keeps after a crop): запасной контур.
- Space (the key): Пробел. Debug log: журнал отладки.
- Start (the laser panel button, not translated yet): «Start».
- Hotkey conflicts: "Ctrl+S уже назначено команде ...", "назначено нескольким командам";
  Take the key: Забрать клавишу.
- Choices agree with their label: Toolbar icons (значки) are Маленькие / Средние / Большие;
  Curve quality (качество) is Низкое / Обычное / Высокое; Theme (тема) is Темная / Светлая.
  The same Dark / Light choices fill the canvas Background row, so that row is the feminine
  Подложка (a camera background capture stays снимок фона).
- Flatten Image Crop: Окончательно обрезать изображение, so it does not read like Apply Crop
  Path (Применить контур обрезки к изображению).
