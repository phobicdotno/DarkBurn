"""Writes DarkBurn's own icons (db_*.svg) in SolidCraft's icon style: 64x64 viewBox, outline
2.55, action stroke 4.25, navigation 5.6, round caps and joins, palette source colours only.
Grey reference geometry, one blue result or action cue. Run from the repository root:

    python tools/make_icons.py
"""
import math
import os

OUT = os.path.join("assets", "icons", "ui", "design", "svg", "model")
INK = "#34465b"
P1, P2, P3 = "#e2e8ef", "#b8c3cf", "#8e9faf"
A1, A2, A3 = "#b9ddf8", "#328bd3", "#236ead"
GUIDE, ORANGE, ORANGE1 = "#6f879c", "#c06f00", "#f1d2af"
STROKE = 'stroke-linejoin="round" stroke-linecap="round"'


def n(v):
    return f"{v:.2f}".rstrip("0").rstrip(".")


def fill(d, c):
    return f'  <path d="{d}" fill="{c}"/>'


def line(d, c=INK, w=2.55, dash=None):
    extra = f' stroke-dasharray="{dash}"' if dash else ""
    return f'  <path d="{d}" fill="none" stroke="{c}" stroke-width="{w}"{extra} {STROKE}/>'


def shape(d, c):
    return [fill(d, c), line(d)]


def circle(cx, cy, r, c=None, stroke=INK, w=2.55):
    out = []
    if c:
        out.append(f'  <circle cx="{cx}" cy="{cy}" r="{r}" fill="{c}"/>')
    if stroke:
        out.append(f'  <circle cx="{cx}" cy="{cy}" r="{r}" fill="none" stroke="{stroke}" stroke-width="{w}" {STROKE}/>')
    return out


def rect(x, y, w, h):
    return f"M{x} {y}L{x + w} {y}L{x + w} {y + h}L{x} {y + h}Z"


def rrect(x, y, w, h, r):
    return (f"M{x + r} {y}L{x + w - r} {y}A{r} {r} 0 0 1 {x + w} {y + r}L{x + w} {y + h - r}"
            f"A{r} {r} 0 0 1 {x + w - r} {y + h}L{x + r} {y + h}A{r} {r} 0 0 1 {x} {y + h - r}"
            f"L{x} {y + r}A{r} {r} 0 0 1 {x + r} {y}Z")


def arrow(x1, y1, x2, y2, c=A3, w=4.25, head=7):
    a = math.atan2(y2 - y1, x2 - x1)
    lx, ly = x2 - head * math.cos(a - 0.6), y2 - head * math.sin(a - 0.6)
    rx, ry = x2 - head * math.cos(a + 0.6), y2 - head * math.sin(a + 0.6)
    return line(f"M{n(x1)} {n(y1)}L{n(x2)} {n(y2)}M{n(lx)} {n(ly)}L{n(x2)} {n(y2)}L{n(rx)} {n(ry)}", c, w)


def svg(title, body):
    head = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">\n' + f"  <title>{title}</title>\n"
    return head + "\n".join(body) + "\n</svg>\n"


ICONS = {}

# Laser head: grey head, blue beam, orange burn spot on the work.
ICONS["db_laser"] = ("Laser", [
    *shape("M20 6L44 6L44 26L38 34L26 34L20 26Z", P2),
    line("M20 18L44 18"),
    line("M32 36L32 50", A3, 4.25),
    line("M12 56L52 56", GUIDE),
    *circle(32, 53, 3.5, ORANGE, None),
])
# Frame: dashed job outline with the head's dot travelling around it.
ICONS["db_frame"] = ("Frame", [
    line(rect(10, 14, 44, 36), GUIDE, 2.55, "5 5"),
    arrow(14, 14, 40, 14),
    *circle(54, 14, 4.5, A2),
])
# Preview: a page with a toolpath and a play mark.
ICONS["db_preview"] = ("Preview", [
    *shape(rrect(8, 8, 40, 48, 4), P1),
    line("M16 20L40 20M16 30L32 30M16 40L28 40", GUIDE),
    *shape("M34 34L58 46L34 58Z", A2),
])
# Zoom in / out: SolidCraft's magnifier weight with a cue.
for key, title, cue in [("db_zoom_in", "Zoom in", "M20 27L34 27M27 20L27 34"), ("db_zoom_out", "Zoom out", "M20 27L34 27")]:
    ICONS[key] = (title, [
        line("M38.5 38.5L54 54", INK, 5.6),
        *circle(27, 27, 17, P1, INK, 4.25),
        line(cue, A3, 4.25),
    ])
ICONS["db_zoom_page"] = ("Zoom to page", [
    *shape(rect(14, 14, 36, 36), P1),
    line("M6 16L6 6L16 6M48 6L58 6L58 16M58 48L58 58L48 58M16 58L6 58L6 48", A3, 4.25),
])
ICONS["db_frame_selection"] = ("Frame selection", [
    line(rect(8, 8, 34, 30), GUIDE, 2.55, "4 4"),
    *shape(rect(14, 14, 22, 18), P2),
    line("M45 45L56 56", INK, 5.6),
    *circle(40, 40, 9, A1, A3, 4.25),
])
# Group and ungroup: two shapes held by a dashed box, or pulled apart.
ICONS["db_group"] = ("Group", [
    line(rect(6, 6, 52, 52), A3, 2.55, "5 5"),
    *shape(rect(14, 14, 18, 18), P2),
    *circle(40, 40, 10, A2),
])
ICONS["db_ungroup"] = ("Ungroup", [
    line("M6 22L6 6L22 6M42 58L58 58L58 42", GUIDE, 2.55, "5 5"),
    *shape(rect(10, 10, 18, 18), P2),
    *circle(44, 44, 10, A2),
    arrow(26, 38, 16, 48, A3, 4.25, 6),
    arrow(38, 26, 48, 16, A3, 4.25, 6),
])
# Flips: grey original, blue mirror image over a dashed axis.
ICONS["db_flip_h"] = ("Flip horizontal", [
    line("M32 6L32 58", GUIDE, 2, "4 4"),
    *shape("M8 50L26 50L26 14Z", P2),
    *shape("M56 50L38 50L38 14Z", A2),
])
ICONS["db_flip_v"] = ("Flip vertical", [
    line("M6 32L58 32", GUIDE, 2, "4 4"),
    *shape("M14 8L14 26L50 26Z", P2),
    *shape("M14 56L14 38L50 38Z", A2),
])
# Alignment: the blue line is the reference the shapes line up on.
ICONS["db_align_centers"] = ("Align centres", [
    *shape(rect(10, 10, 30, 16), P2),
    *shape(rect(24, 38, 30, 16), P2),
    line("M32 4L32 60M4 32L60 32", A3, 4.25),
])
ICONS["db_align_v"] = ("Align vertical centres", [
    *shape(rect(12, 10, 40, 16), P2),
    *shape(rect(20, 38, 24, 16), P2),
    line("M32 4L32 60", A3, 4.25),
])
ICONS["db_align_h"] = ("Align horizontal centres", [
    *shape(rect(10, 12, 16, 40), P2),
    *shape(rect(38, 20, 16, 24), P2),
    line("M4 32L60 32", A3, 4.25),
])
ICONS["db_distribute_h"] = ("Distribute horizontally", [
    line("M6 6L6 58M58 6L58 58", GUIDE),
    *shape(rect(12, 22, 10, 20), P2),
    *shape(rect(27, 14, 10, 36), A2),
    *shape(rect(42, 22, 10, 20), P2),
])
ICONS["db_distribute_v"] = ("Distribute vertically", [
    line("M6 6L58 6M6 58L58 58", GUIDE),
    *shape(rect(22, 12, 20, 10), P2),
    *shape(rect(14, 27, 36, 10), A2),
    *shape(rect(22, 42, 20, 10), P2),
])
ICONS["db_same_width"] = ("Make same width", [
    *shape(rect(10, 8, 44, 16), P2),
    *shape(rect(10, 32, 44, 16), A2),
    line("M10 56L54 56M10 52L10 60M54 52L54 60", A3),
])
ICONS["db_same_height"] = ("Make same height", [
    *shape(rect(8, 10, 16, 44), P2),
    *shape(rect(32, 10, 16, 44), A2),
    line("M56 10L56 54M52 10L60 10M52 54L60 54", A3),
])
# Boolean operations on a square and a circle; blue is what remains.
SQ, CI = rect(8, 8, 32, 32), "M24 40A16 16 0 1 0 56 40A16 16 0 1 0 24 40Z"
UNION = "M40 24.4A16 16 0 0 1 40 56A16 16 0 0 1 24 40L8 40L8 8L40 8Z"
DIFF = "M8 8L40 8L40 24.4A16 16 0 0 0 24 40L8 40Z"
INTER = "M40 24.4L40 40L24 40A16 16 0 0 1 40 24.4Z"
ICONS["db_union"] = ("Boolean union", [fill(SQ, A2), fill(CI, A2), line(UNION)])
ICONS["db_difference"] = ("Boolean difference", [*shape(DIFF, A2), line(CI, GUIDE, 2.55, "4 4")])
ICONS["db_intersection"] = ("Boolean intersection", [
    line(SQ, GUIDE, 2.55, "4 4"), line(CI, GUIDE, 2.55, "4 4"), *shape(INTER, A2),
])
ICONS["db_weld"] = ("Weld", [
    fill("M8 32A16 16 0 1 0 40 32A16 16 0 1 0 8 32Z", P2),
    fill("M24 32A16 16 0 1 0 56 32A16 16 0 1 0 24 32Z", P2),
    line("M32 18.14A16 16 0 1 1 32 45.86A16 16 0 1 1 32 18.14Z"),
    line("M24 56L40 56M24 8L40 8", A3, 4.25),
])
# Docking: the shape moves to a wall.
for key, title, wall, sq, arr in [
    ("db_dock_left", "Dock left", "M8 6L8 58", rect(28, 22, 20, 20), (56, 32, 16, 32)),
    ("db_dock_right", "Dock right", "M56 6L56 58", rect(16, 22, 20, 20), (8, 32, 48, 32)),
    ("db_dock_up", "Dock up", "M6 8L58 8", rect(22, 28, 20, 20), (32, 56, 32, 16)),
    ("db_dock_down", "Dock down", "M6 56L58 56", rect(22, 16, 20, 20), (32, 8, 32, 48)),
]:
    ICONS[key] = (title, [line(wall, INK, 4.25), *shape(sq, P2), arrow(*arr, A3, 4.25, 6)])
# Rotary: a cylinder turning about its axis.
ICONS["db_rotary"] = ("Rotary", [
    fill("M14 18L46 18L46 46L14 46Z", P2),
    fill("M46 18A6 14 0 0 1 46 46A6 14 0 0 1 46 18Z", P1),
    line("M14 18L46 18M14 46L46 46M14 18A6 14 0 0 0 14 46M46 18A6 14 0 0 1 46 46A6 14 0 0 1 46 18Z"),
    line("M4 32L60 32", GUIDE, 2, "4 4"),
    line("M22 9A22 9 0 0 1 44 9", A3, 4.25),
    line("M44 9L38 4M44 9L38 13", A3, 4.25),
])
ICONS["db_cylinder"] = ("Cylinder correction", [
    fill("M16 14A16 6 0 0 1 48 14L48 50A16 6 0 0 1 16 50Z", P2),
    fill("M16 14A16 6 0 0 1 48 14A16 6 0 0 1 16 14Z", P1),
    line("M16 14A16 6 0 0 1 48 14A16 6 0 0 1 16 14ZM16 14L16 50A16 6 0 0 0 48 50L48 14"),
    line("M24 25L24 54M32 26L32 56M40 25L40 54", A3),
])
# Print and cut: two registration targets.
ICONS["db_print_cut"] = ("Print and cut", [
    *shape(rrect(6, 6, 52, 52, 4), P1),
    *circle(20, 20, 6, None, A3), line("M20 10L20 30M10 20L30 20", A3),
    *circle(44, 44, 6, None, A3), line("M44 34L44 54M34 44L54 44", A3),
])
# Tools column.
ICONS["db_node_edit"] = ("Edit nodes", [
    line("M10 50L24 26M54 14L36 30", GUIDE, 2, "3 3"),
    line("M10 50C10 20 30 12 54 14", INK, 4.25),
    *shape(rect(6, 46, 8, 8), A2),
    *shape(rect(50, 10, 8, 8), A2),
    *circle(24, 26, 3, P1), *circle(36, 30, 3, P1),
])
ICONS["db_tabs"] = ("Add tabs", [
    line("M18.8 9.1A26 26 0 0 1 45.2 9.1M54.9 18.8A26 26 0 0 1 54.9 45.2"
         "M45.2 54.9A26 26 0 0 1 18.8 54.9M9.1 45.2A26 26 0 0 1 9.1 18.8", INK, 4.25),
    line("M50 13L56 7M7 57L13 51M51 51L57 57M7 7L13 13", A3, 4.25),
])
ICONS["db_position_laser"] = ("Position laser", [
    *circle(32, 32, 18),
    line("M32 4L32 22M32 42L32 60M4 32L22 32M42 32L60 32", INK, 4.25),
    *circle(32, 32, 5, ORANGE, None),
])
ICONS["db_qr"] = ("Create QR code", [
    *shape(rect(6, 6, 20, 20), P1), fill(rect(12, 12, 8, 8), INK),
    *shape(rect(38, 6, 20, 20), P1), fill(rect(44, 12, 8, 8), INK),
    *shape(rect(6, 38, 20, 20), P1), fill(rect(12, 44, 8, 8), INK),
    fill(rect(38, 38, 8, 8), A3), fill(rect(50, 38, 8, 8), A3), fill(rect(44, 46, 8, 6), A3),
    fill(rect(38, 52, 8, 6), A3), fill(rect(52, 52, 6, 6), A3),
])
ICONS["db_device_settings"] = ("Device settings", [
    *shape("M8 10L56 10L56 22L8 22Z", P2),
    *shape("M24 22L40 22L36 32L28 32Z", P1),
    line("M32 34L32 44", A3, 4.25),
    line("M8 54L56 54", GUIDE),
    *circle(32, 49, 3, ORANGE, None),
])
ICONS["db_start_points"] = ("Start points", [
    line("M14 50L14 20A6 6 0 0 1 20 14L50 14", INK, 4.25),
    *circle(14, 50, 6, A2),
    arrow(30, 14, 46, 14, A3, 4.25, 6),
])
ICONS["db_round_corners"] = ("Round corners", [
    line("M12 24L12 12L24 12", GUIDE, 2.55, "3 4"),
    line("M12 54L12 32A20 20 0 0 1 32 12L54 12", A3, 4.25),
])
ICONS["db_console"] = ("Console", [
    *shape(rrect(6, 10, 52, 44, 4), P1),
    line("M14 24L22 30L14 36", A3, 4.25),
    line("M28 38L40 38", INK, 4.25),
])
ICONS["db_material_test"] = ("Material test", [
    *shape(rect(6, 6, 52, 52), P1),
    fill(rect(12, 12, 10, 10), P2), fill(rect(27, 12, 10, 10), P3), fill(rect(42, 12, 10, 10), INK),
    fill(rect(12, 27, 10, 10), A1), fill(rect(27, 27, 10, 10), A2), fill(rect(42, 27, 10, 10), A3),
    fill(rect(12, 42, 10, 10), ORANGE1), fill(rect(27, 42, 10, 10), ORANGE), fill(rect(42, 42, 10, 10), INK),
])
ICONS["db_capture"] = ("Capture image", [
    *shape("M6 22L18 22L22 14L42 14L46 22L58 22L58 52L6 52Z", P2),
    *circle(32, 36, 10, A1),
    *circle(32, 36, 4, A3, None),
])
# Application icon: an engraving slab with a blue beam and an orange burn line.
ICONS["db_app"] = ("DarkBurn", [
    fill("M32 26L58 41L32 56L6 41Z", P1),
    fill("M6 41L32 56L32 61L6 46Z", P2),
    fill("M58 41L32 56L32 61L58 46Z", P3),
    line("M32 26L58 41L32 56L6 41ZM6 41L6 46L32 61L58 46L58 41M32 56L32 61"),
    line("M18 43L32 35L44 42", ORANGE, 4.25),
    *shape("M23 3L41 3L41 12L36 19L28 19L23 12Z", P2),
    line("M32 21L32 33", A3, 4.25),
    *circle(32, 36, 3.2, ORANGE, None),
])

if __name__ == "__main__":
    for key, (title, body) in ICONS.items():
        with open(os.path.join(OUT, key + ".svg"), "w", newline="\n") as f:
            f.write(svg(title, body))
    print(len(ICONS), "icons")
