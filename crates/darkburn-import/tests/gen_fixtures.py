"""Generates the small hand-authored fixtures in tests/data.

Run from the crate directory: python tests/gen_fixtures.py tests/data
Needs Pillow (for the JPEG, PNG and CCITT G4 samples).
"""
import base64
import io
import os
import sys
import zlib

from PIL import Image

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "data")
os.makedirs(OUT, exist_ok=True)


def w(name, data):
    if isinstance(data, str):
        data = data.encode("latin-1")
    with open(os.path.join(OUT, name), "wb") as f:
        f.write(data)


# ---------------------------------------------------------------- DXF
def pairs(*items):
    out = []
    for code, value in items:
        out.append(f"{code:>3}")
        out.append(str(value))
    return out


def dxf_doc(header_vars, tables, blocks, entities):
    lines = []
    lines += pairs((0, "SECTION"), (2, "HEADER"))
    lines += pairs((9, "$ACADVER"), (1, "AC1009"))
    for name, code, value in header_vars:
        lines += pairs((9, name), (code, value))
    lines += pairs((0, "ENDSEC"))
    lines += pairs((0, "SECTION"), (2, "TABLES"))
    lines += tables
    lines += pairs((0, "ENDSEC"))
    lines += pairs((0, "SECTION"), (2, "BLOCKS"))
    lines += blocks
    lines += pairs((0, "ENDSEC"))
    lines += pairs((0, "SECTION"), (2, "ENTITIES"))
    lines += entities
    lines += pairs((0, "ENDSEC"), (0, "EOF"))
    return "\n".join(lines) + "\n"


def line(x1, y1, x2, y2, layer="0", extra=()):
    return pairs((0, "LINE"), (8, layer), *extra, (10, x1), (20, y1), (30, 0), (11, x2), (21, y2), (31, 0))


def layer_rec(name, color, flags=0, ltype="CONTINUOUS", truecolor=None):
    items = [(0, "LAYER"), (2, name), (70, flags), (62, color), (6, ltype)]
    if truecolor is not None:
        items.append((420, truecolor))
    return pairs(*items)


def table(name, recs):
    return pairs((0, "TABLE"), (2, name), (70, len(recs))) + [l for r in recs for l in r] + pairs((0, "ENDTAB"))


ltypes = table("LTYPE", [
    pairs((0, "LTYPE"), (2, "CONTINUOUS"), (70, 0), (3, "Solid"), (72, 65), (73, 0), (40, 0.0)),
    pairs((0, "LTYPE"), (2, "DASHED"), (70, 0), (3, "Dashed"), (72, 65), (73, 2), (40, 0.75), (49, 0.5), (49, -0.25)),
])
layers = table("LAYER", [
    layer_rec("0", 7),
    layer_rec("CUT", 1),
    layer_rec("TRUE", 3, truecolor=0x123456),
    layer_rec("FROZEN", 5, flags=1),
    layer_rec("OFF", -4),
    layer_rec("DASH", 2, ltype="DASHED"),
])
styles = table("STYLE", [
    pairs((0, "STYLE"), (2, "STANDARD"), (70, 0), (40, 0.0), (41, 1.0), (50, 0.0), (71, 0), (42, 2.5), (3, "txt"), (4, "")),
    pairs((0, "STYLE"), (2, "ARIALSTYLE"), (70, 0), (40, 0.0), (41, 1.0), (50, 0.0), (71, 0), (42, 2.5), (3, "arial.ttf"), (4, "")),
])

blocks = []
# Inner block: a 1x1 square on layer 0 (takes the insert's layer) with ByBlock colour.
blocks += pairs((0, "BLOCK"), (8, "0"), (2, "INNER"), (70, 0), (10, 0), (20, 0), (30, 0), (3, "INNER"))
blocks += pairs((0, "LWPOLYLINE"), (8, "0"), (62, 0), (90, 4), (70, 1),
                (10, 0), (20, 0), (10, 1), (20, 0), (10, 1), (20, 1), (10, 0), (20, 1))
blocks += pairs((0, "ENDBLK"), (8, "0"))
# Outer block: base point (5,5), inserts INNER at (5,5) scaled 2.
blocks += pairs((0, "BLOCK"), (8, "0"), (2, "OUTER"), (70, 2), (10, 5), (20, 5), (30, 0), (3, "OUTER"))
blocks += pairs((0, "INSERT"), (8, "0"), (2, "INNER"), (10, 5), (20, 5), (30, 0), (41, 2), (42, 2), (43, 1), (50, 0))
blocks += pairs((0, "ATTDEF"), (8, "0"), (10, 0), (20, 0), (30, 0), (40, 1), (1, "x"), (3, "Prompt"), (2, "TAG"), (70, 0))
blocks += pairs((0, "ENDBLK"), (8, "0"))

ents = []
ents += line(0, 0, 10, 0, "CUT")
# Bulged LWPOLYLINE: half circle from (20,0) to (30,0) (bulge 1), closed back with a line.
ents += pairs((0, "LWPOLYLINE"), (8, "CUT"), (90, 2), (70, 1), (10, 20), (20, 0), (42, 1.0), (10, 30), (20, 0))
ents += pairs((0, "CIRCLE"), (8, "TRUE"), (10, 50), (20, 50), (30, 0), (40, 5))
ents += pairs((0, "ARC"), (8, "CUT"), (62, 5), (10, 0), (20, 50), (30, 0), (40, 10), (50, 0), (51, 90))
ents += pairs((0, "ELLIPSE"), (8, "CUT"), (10, 100), (20, 0), (30, 0), (11, 20), (21, 0), (31, 0), (40, 0.5), (41, 0), (42, 6.283185307179586))
# Clamped cubic B-spline with 5 control points (2 Bezier spans).
spline = [(0, "SPLINE"), (8, "CUT"), (70, 8), (71, 3), (72, 9), (73, 5), (74, 0)]
for k in [0, 0, 0, 0, 1, 2, 2, 2, 2]:
    spline.append((40, k))
for (x, y) in [(0, 100), (10, 120), (20, 100), (30, 120), (40, 100)]:
    spline += [(10, x), (20, y), (30, 0)]
ents += pairs(*spline)
ents += pairs((0, "TEXT"), (8, "CUT"), (10, 5), (20, 200), (30, 0), (40, 3.5), (1, "Hello %%d"), (50, 90), (7, "ARIALSTYLE"))
ents += pairs((0, "MTEXT"), (8, "CUT"), (10, 5), (20, 220), (30, 0), (40, 2.5), (71, 1), (1, "{\\fArial|b1;Line one}\\PLine two"), (7, "STANDARD"))
ents += line(0, 0, 1, 1, "FROZEN")
ents += line(0, 0, 1, 1, "OFF")
ents += line(0, 300, 10, 300, "DASH")
ents += pairs((0, "INSERT"), (8, "CUT"), (66, 1), (2, "OUTER"), (10, 200), (20, 200), (30, 0), (41, 1), (42, 1), (43, 1), (50, 90))
ents += pairs((0, "ATTRIB"), (8, "CUT"), (10, 210), (20, 190), (30, 0), (40, 2), (1, "PART-7"), (2, "TAG"), (70, 0))
ents += pairs((0, "SEQEND"), (8, "CUT"))
# Two open lines whose ends nearly touch (for auto-join).
ents += line(500, 0, 510, 0, "CUT")
ents += line(510.005, 0, 510, 10, "CUT")

w("entities.dxf", dxf_doc([("$INSUNITS", 70, 4)], ltypes + layers + styles, blocks, ents))
for name, code in [("inches", 1), ("centimetres", 5), ("metres", 6), ("kilometres", 7)]:
    w(f"units_{name}.dxf", dxf_doc([("$INSUNITS", 70, code)], table("LAYER", [layer_rec("0", 7)]), [], line(0, 0, 1, 0)))
w("units_none.dxf", dxf_doc([], table("LAYER", [layer_rec("0", 7)]), [], line(0, 0, 1, 0)))
w("units_measurement_english.dxf", dxf_doc([("$MEASUREMENT", 70, 0)], table("LAYER", [layer_rec("0", 7)]), [], line(0, 0, 1, 0)))
# Entities only, no tables at all.
w("minimal.dxf", "\n".join(pairs((0, "SECTION"), (2, "ENTITIES")) + line(0, 0, 3, 4) + pairs((0, "ENDSEC"), (0, "EOF"))) + "\n")


# ---------------------------------------------------------------- PDF
class Pdf:
    def __init__(self):
        self.objs = []

    def add(self, body):
        self.objs.append(body if isinstance(body, bytes) else body.encode("latin-1"))
        return len(self.objs)

    def reserve(self):
        self.objs.append(None)
        return len(self.objs)

    def set(self, n, body):
        self.objs[n - 1] = body if isinstance(body, bytes) else body.encode("latin-1")

    def stream(self, dict_body, data, compress=False):
        if compress:
            data = zlib.compress(data)
            dict_body += " /Filter /FlateDecode"
        return self.add(b"<< " + dict_body.encode("latin-1") + b" /Length " + str(len(data)).encode() + b" >>\nstream\n" + data + b"\nendstream")

    def build(self, root):
        out = bytearray(b"%PDF-1.5\n%\xe2\xe3\xcf\xd3\n")
        offsets = []
        for i, body in enumerate(self.objs):
            offsets.append(len(out))
            out += f"{i + 1} 0 obj\n".encode() + body + b"\nendobj\n"
        xref = len(out)
        out += f"xref\n0 {len(self.objs) + 1}\n0000000000 65535 f \n".encode()
        for o in offsets:
            out += f"{o:010d} 00000 n \n".encode()
        out += f"trailer\n<< /Size {len(self.objs) + 1} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
        return bytes(out)


def vector_pdf():
    p = Pdf()
    pages = p.reserve()
    ocg_on = p.add("<< /Type /OCG /Name (Cut lines) >>")
    ocg_off = p.add("<< /Type /OCG /Name (Hidden stuff) >>")
    form = p.stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [1 0 0 1 100 0]",
                    b"0 0 1 RG 0 0 m 72 0 l S")
    content1 = b"\n".join([
        b"q 2 0 0 2 0 0 cm",
        b"1 0 0 RG 0 0 36 36 re S",                       # red stroked square, 72 pt = 25.4 mm
        b"Q",
        b"0 1 1 0 k 100 100 m 172 100 l 172 172 l h f",   # CMYK fill (m=1 y=1) -> red
        b"0.5 g 200 200 m 210 260 250 260 260 200 c f",   # gray fill curve
        b"[3 2] 0 d 0 G 300 300 m 372 300 l S [] 0 d",    # dashed line
        b"q 0 0 10 10 re W n 0 0 m 5 5 l S Q",            # clip path
        b"/OC /L1 BDC 0 1 0 RG 400 400 m 472 400 l S EMC",
        b"/OC /L2 BDC 0 0 0 RG 500 500 m 572 500 l S EMC",
        b"/Fm1 Do",
        b"BT /F1 12 Tf 10 10 Td (text) Tj ET",
    ])
    c1 = p.stream("", content1, compress=True)
    page1 = p.add(f"<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] /Contents {c1} 0 R "
                  f"/Resources << /XObject << /Fm1 {form} 0 R >> /Properties << /L1 {ocg_on} 0 R /L2 {ocg_off} 0 R >> >> >>")
    c2 = p.stream("", b"0 0 1 RG 0 0 m 72 72 l S")
    page2 = p.add(f"<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 200 100] /Rotate 90 /Contents {c2} 0 R >>")
    p.set(pages, f"<< /Type /Pages /Kids [{page1} 0 R {page2} 0 R] /Count 2 >>")
    root = p.add(f"<< /Type /Catalog /Pages {pages} 0 R /OCProperties << /OCGs [{ocg_on} 0 R {ocg_off} 0 R] "
                 f"/D << /ON [{ocg_on} 0 R] /OFF [{ocg_off} 0 R] >> >> >>")
    return p.build(root)


def images_pdf():
    p = Pdf()
    pages = p.reserve()
    # 2x2 RGB Flate image: red, green / blue, white
    rgb = bytes([255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255])
    im_rgb = p.stream("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8", rgb, compress=True)
    # 8x2 1-bit gray: first row alternating, second row all white
    im_bits = p.stream("/Type /XObject /Subtype /Image /Width 8 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 1",
                       bytes([0b10101010, 0b11111111]))
    # 8x1 stencil mask painted in the current fill colour
    im_mask = p.stream("/Type /XObject /Subtype /Image /Width 8 /Height 1 /ImageMask true", bytes([0b00001111]))
    # Indexed 2-colour palette image 2x1
    im_idx = p.stream("/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace [/Indexed /DeviceRGB 1 <FF000000FF00>] /BitsPerComponent 8",
                      bytes([0, 1]))
    # JPEG 4x4 solid blue
    jb = io.BytesIO()
    Image.new("RGB", (4, 4), (0, 0, 255)).save(jb, format="JPEG", quality=95)
    im_jpg = p.stream("/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode", jb.getvalue())
    # CCITT G4 16x4: left half black
    bw = Image.new("1", (16, 4), 1)
    for y in range(4):
        for x in range(8):
            bw.putpixel((x, y), 0)
    tb = io.BytesIO()
    bw.save(tb, format="TIFF", compression="group4")
    t = Image.open(io.BytesIO(tb.getvalue()))
    off = t.tag_v2[273][0]
    cnt = t.tag_v2[279][0]
    # Pillow writes the TIFF as WhiteIsZero, so its black pixels are CCITT "white" runs;
    # /BlackIs1 true makes those samples 0 (black) in DeviceGray again.
    g4 = tb.getvalue()[off:off + cnt]
    im_g4 = p.stream("/Type /XObject /Subtype /Image /Width 16 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 1 "
                     "/Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 16 /Rows 4 /BlackIs1 true >>", g4)
    content = b"\n".join([
        b"q 72 0 0 72 0 0 cm /ImRGB Do Q",
        b"q 72 0 0 18 100 0 cm /ImBits Do Q",
        b"1 0 0 rg q 72 0 0 9 200 0 cm /ImMask Do Q",
        b"q 20 0 0 10 300 0 cm /ImIdx Do Q",
        b"q 40 0 0 40 0 100 cm /ImJpg Do Q",
        b"q 32 0 0 8 100 100 cm /ImG4 Do Q",
        b"q 20 0 0 10 200 100 cm BI /W 2 /H 1 /CS /RGB /BPC 8 ID \xff\x00\x00\x00\x00\xff EI Q",
    ])
    c = p.stream("", content)
    page = p.add(f"<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 400 300] /Contents {c} 0 R /Resources << /XObject << "
                 f"/ImRGB {im_rgb} 0 R /ImBits {im_bits} 0 R /ImMask {im_mask} 0 R /ImIdx {im_idx} 0 R /ImJpg {im_jpg} 0 R /ImG4 {im_g4} 0 R >> >> >>")
    p.set(pages, f"<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>")
    root = p.add(f"<< /Type /Catalog /Pages {pages} 0 R >>")
    return p.build(root)


vp = vector_pdf()
w("vector.pdf", vp)
w("pdf_compatible.ai", vp)
w("images.pdf", images_pdf())
w("legacy_postscript.ai", "%!PS-Adobe-3.0\n%%Creator: Adobe Illustrator(R) 8.0\n%%BoundingBox: 0 0 100 100\n%%EndComments\n"
  "0 0 m 100 100 L S\n%%EOF\n")

# ---------------------------------------------------------------- LightBurn
png = io.BytesIO()
Image.new("RGB", (2, 2), (255, 255, 255)).save(png, format="PNG")
img = Image.new("RGB", (2, 2), (0, 0, 0))
img.putpixel((0, 0), (255, 0, 0))  # top-left red
pb = io.BytesIO()
img.save(pb, format="PNG")
thumb = base64.b64encode(png.getvalue()).decode()
bitmap = base64.b64encode(pb.getvalue()).decode()

lbrn2 = f"""<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="1.4.04" FormatVersion="1" MaterialHeight="3" MirrorX="False" MirrorY="True">
    <Thumbnail Source="{thumb}"/>
    <VariableText>
        <Start Value="0"/>
        <End Value="999"/>
        <Current Value="5"/>
        <Increment Value="1"/>
        <AutoAdvance Value="0"/>
    </VariableText>
    <UIPrefs>
        <Optimize_ByLayer Value="0"/>
        <Optimize_ByGroup Value="-1"/>
    </UIPrefs>
    <CutSetting type="Cut">
        <index Value="0"/>
        <name Value="Outline"/>
        <minPower Value="15"/>
        <maxPower Value="60"/>
        <speed Value="20"/>
        <numPasses Value="2"/>
        <priority Value="1"/>
    </CutSetting>
    <CutSetting type="Scan">
        <index Value="1"/>
        <name Value="Engrave"/>
        <maxPower Value="30"/>
        <speed Value="200"/>
        <interval Value="0.1"/>
        <angle Value="45"/>
    </CutSetting>
    <CutSetting_Img type="Image">
        <index Value="2"/>
        <name Value="Photo"/>
        <maxPower Value="25"/>
        <speed Value="150"/>
        <ditherMode Value="stucki"/>
    </CutSetting_Img>
    <Shape Type="Rect" CutIndex="0" W="20" H="10" Cr="2">
        <XForm>1 0 0 1 50 50</XForm>
    </Shape>
    <Shape Type="Ellipse" CutIndex="0" Rx="5" Ry="3">
        <XForm>1 0 0 1 100 100</XForm>
    </Shape>
    <Shape Type="Path" CutIndex="1" VertID="0" PrimID="0">
        <XForm>1 0 0 1 0 0</XForm>
        <VertList>V0 0c0x1c1x1V10 0c0x15c0y5c1x1V10 10c0x1c1x10c1y15V0 10c0x1c1x1</VertList>
        <PrimList>L0 1B1 2L2 3L3 0</PrimList>
    </Shape>
    <Shape Type="Path" CutIndex="1" VertID="0" PrimID="0">
        <XForm>1 0 0 1 200 0</XForm>
    </Shape>
    <Shape Type="Path" CutIndex="0">
        <XForm>1 0 0 1 300 0</XForm>
        <VertList>V0 0V5 0V5 5</VertList>
        <PrimList>LineClosed</PrimList>
    </Shape>
    <Shape Type="Polygon" CutIndex="0" N="6" Rx="10" Ry="10">
        <XForm>1 0 0 1 400 0</XForm>
    </Shape>
    <Shape Type="Group">
        <XForm>1 0 0 1 1000 0</XForm>
        <Children>
            <Shape Type="Rect" CutIndex="0" W="2" H="2" Cr="0">
                <XForm>1 0 0 1 1 1</XForm>
            </Shape>
            <Shape Type="Group">
                <XForm>2 0 0 2 0 0</XForm>
                <Children>
                    <Shape Type="Rect" CutIndex="0" W="2" H="2" Cr="0">
                        <XForm>1 0 0 1 10 0</XForm>
                    </Shape>
                </Children>
            </Shape>
        </Children>
    </Shape>
    <Shape Type="Text" CutIndex="1" Str="Hi" Font="Century Gothic,-1,100,5,50,0,0,0,0,0" H="8" LS="0" LnS="0" Ah="1" Av="0" HasBackupPath="1">
        <XForm>1 0 0 1 0 500</XForm>
        <BackupPath Type="Path" CutIndex="1">
            <XForm>1 0 0 1 0 0</XForm>
            <VertList>V0 0V4 0V4 8V0 8</VertList>
            <PrimList>LineClosed</PrimList>
        </BackupPath>
    </Shape>
    <Shape Type="Bitmap" CutIndex="2" W="20" H="20" Gamma="1" Contrast="0" Brightness="0" DPI="254" Data="{bitmap}">
        <XForm>0.5 0 0 0.5 600 600</XForm>
    </Shape>
    <Notes ShowOnLoad="1" Notes="Use 3 mm birch&#10;Second line"/>
</LightBurnProject>
"""
w("project.lbrn2", lbrn2)

lbrn = """<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="0.9.16" FormatVersion="0" MaterialHeight="0" MirrorX="False" MirrorY="True">
    <CutSetting type="Cut">
        <index Value="3"/>
        <name Value="C03"/>
        <maxPower Value="70"/>
        <speed Value="10"/>
    </CutSetting>
    <Shape Type="Path" CutIndex="3">
        <XForm>1 0 0 1 10 20</XForm>
        <V vx="0" vy="0"/>
        <V vx="10" vy="0" c0x="10" c0y="5"/>
        <V vx="10" vy="10" c1x="15" c1y="10"/>
        <P T="L" p0="0" p1="1"/>
        <P T="B" p0="1" p1="2"/>
        <P T="L" p0="2" p1="0"/>
    </Shape>
    <Shape Type="Rect" CutIndex="3" W="4" H="4" Cr="0">
        <XForm>0 1 -1 0 0 0</XForm>
    </Shape>
</LightBurnProject>
"""
w("legacy.lbrn", lbrn)

# ---------------------------------------------------------------- HPGL
w("sample.plt", "IN;SP1;PU0,0;PD400,0,400,400,0,400,0,0;PU;SP2;PU800,800;CI200;"
  "PU1000,0;PR;PD40,0,0,40;PA;PU2000,0;PD;AA2000,400,90;PU;EA0,0;"
  "LBhello\x03PU;SP0;\n")

# ---------------------------------------------------------------- SignCut
w("design.sc", b"SCFILE\x00\x01\x02 not documented")
print("ok")
