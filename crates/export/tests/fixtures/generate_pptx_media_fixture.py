"""Build a synthetic deck with a large, reused raster background.

Uses only Python's standard library. The uncompressed PNG exceeds the old
2 MiB media limit, while the ZIP stays small. Contains no user document data.
"""
from pathlib import Path
from struct import pack
from zipfile import ZipFile, ZipInfo, ZIP_DEFLATED
import binascii
import zlib

HERE = Path(__file__).parent
A = 'http://schemas.openxmlformats.org/drawingml/2006/main'
P = 'http://schemas.openxmlformats.org/presentationml/2006/main'
R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'


def chunk(kind, data):
    return pack('>I', len(data)) + kind + data + pack('>I', binascii.crc32(kind + data) & 0xffffffff)


def background():
    rows = []
    for y in range(720):
        row = bytearray([0])
        for x in range(1280):
            row.extend((233, 136, 42) if x < 32 or y > 680 else (18, 36, 60))
        rows.append(row)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', pack('>IIBBBBB', 1280, 720, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(b''.join(rows), level=0)) + chunk(b'IEND', b''))


def relationships(items):
    return ('<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            + ''.join(f'<Relationship Id="{key}" Type="{R}/{kind}" Target="{target}"/>'
                      for key, kind, target in items) + '</Relationships>')


def slide(index):
    return f'''<p:sld xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}"><p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
<p:pic><p:nvPicPr><p:cNvPr id="2" name="Background" descr="Reusable navy background"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="picture"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="12192000" cy="6858000"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr></p:pic>
<p:sp><p:nvSpPr><p:cNvPr id="3" name="Title"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="762000" y="762000"/><a:ext cx="10668000" cy="1143000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr wrap="square" lIns="0" rIns="0" tIns="0" bIns="0"/><a:lstStyle/><a:p><a:pPr algn="ctr"/><a:r><a:rPr sz="3200" b="1"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:latin typeface="Arial"/></a:rPr><a:t>Reusable background {index}</a:t></a:r></a:p></p:txBody></p:sp>
<p:sp><p:nvSpPr><p:cNvPr id="4" name="Wrapped text"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="762000" y="2286000"/><a:ext cx="2857500" cy="2857500"/></a:xfrm></p:spPr><p:txBody><a:bodyPr wrap="square" lIns="0" rIns="0" tIns="0" bIns="0"/><a:lstStyle/><a:p><a:pPr algn="r"><a:lnSpc><a:spcPct val="125000"/></a:lnSpc><a:spcAft><a:spcPts val="1200"/></a:spcAft></a:pPr><a:r><a:rPr sz="2000"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:latin typeface="Arial"/></a:rPr><a:t>These words must wrap inside the narrow text box and preserve right alignment.</a:t></a:r></a:p></p:txBody></p:sp>
</p:spTree></p:cSld></p:sld>'''


def generate():
    parts = {
        '[Content_Types].xml': '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>' + ''.join(f'<Override PartName="/ppt/slides/slide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>' for i in range(1, 21)) + '</Types>',
        '_rels/.rels': relationships([('presentation', 'officeDocument', 'ppt/presentation.xml')]),
        'ppt/presentation.xml': f'<p:presentation xmlns:p="{P}" xmlns:r="{R}"><p:sldIdLst>' + ''.join(f'<p:sldId id="{255+i}" r:id="slide{i}"/>' for i in range(1, 21)) + '</p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/></p:presentation>',
        'ppt/_rels/presentation.xml.rels': relationships([(f'slide{i}', 'slide', f'slides/slide{i}.xml') for i in range(1, 21)]),
        'ppt/media/background.png': background(),
    }
    assert len(parts['ppt/media/background.png']) > 2 * 1024 * 1024
    for i in range(1, 21):
        parts[f'ppt/slides/slide{i}.xml'] = slide(i)
        parts[f'ppt/slides/_rels/slide{i}.xml.rels'] = relationships([('picture', 'image', '../media/background.png')])
    with ZipFile(HERE / 'reused-large-background.pptx', 'w') as output:
        for path, value in parts.items():
            entry = ZipInfo(path, (2026, 1, 1, 0, 0, 0))
            entry.compress_type = ZIP_DEFLATED
            output.writestr(entry, value)


if __name__ == '__main__':
    generate()
