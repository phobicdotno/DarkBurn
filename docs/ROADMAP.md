# DarkBurn roadmap

Build order for reaching the parity checklist. Each phase ends with a working build and ticked checklist items.

| Phase | Goal | Main checklist sections |
|---|---|---|
| 1 | Window, theme, panels and an empty canvas you can zoom and pan | SHELL, WIN, TB, SB, PANEL, CANVAS (view parts) |
| 2 | Draw and edit basic shapes, layers, save and open projects, SVG and image import | DRAW, LAYER, MODE, CUTS, SP, PROJ, IMP-01, IMP-09, EDIT, ARR basics |
| 3 | First real burn: G-code output and GRBL control of the Atomstack | GRBL, CONN-01, LASER, MOVE, CON, ORIG, EXP-06, TARGET-01 |
| 4 | Fill and image engraving, preview with time estimate, cut planner | FILL, IMGL, IMG, PREV, PLAN |
| 5 | Geometry tools: booleans, offset, weld, node editing, arrays | SHAPE, NODE, ARRAY, TOOLS |
| 6 | File formats: DXF, PDF, Illustrator, PLT, LightBurn projects, exports | IMP, EXP, PROJ, AUX |
| 7 | Gweike M3 Ultra by file export, then the controller card simulator | MCC-01 to MCC-05 |
| 8 | Ruida over Ethernet and USB, tested against an emulator | RUIDA, FL, CONN-04, CONN-06 |
| 9 | Library, material test, variable text, text tools, QR codes | LIB, MT, VT, VTP, DRAW text items |
| 10 | Camera, rotary, print and cut, other tools | CAM, CAMP, ROT, PNC, OTHER, LT |
| 11 | Settings, device settings, hotkeys, languages, platform polish | SET, DS, KEY, I18N, PLAT, HELP |
| 12 | Trocen, TopWisdom, galvo, other G-code firmwares | TROCEN, TOPW, GALVO, GCODE |
| 13 | Native Gweike controller backend, once the protocol is confirmed on the real card | MCC-06, MCC-07, TARGET-03 |

Phase 3 is the first point where DarkBurn can replace LightBurn for the Atomstack.
