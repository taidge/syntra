"""Hand-authored ECMA-376 standard parts; never invokes Contrix exporters.

Run from any cwd with Python's standard library. Fixtures intentionally contain
both representable features and explicit unsupported features. No customXml.
"""
from pathlib import Path
from zipfile import ZipFile, ZipInfo, ZIP_DEFLATED
import base64

HERE = Path(__file__).parent
S = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main'
A = 'http://schemas.openxmlformats.org/drawingml/2006/main'
P = 'http://schemas.openxmlformats.org/presentationml/2006/main'
C = 'http://schemas.openxmlformats.org/drawingml/2006/chart'
R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'

def rels(*items):
    return '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">' + ''.join(
        f'<Relationship Id="{key}" Type="{R}/{kind}" Target="{target}"' + (' TargetMode="External"' if external else '') + '/>'
        for key,kind,target,external in items) + '</Relationships>'

def archive(name, parts):
    with ZipFile(HERE / name, 'w', ZIP_DEFLATED) as z:
        for path, value in parts.items():
            z.writestr(path, value)

THEME = f'''<a:theme xmlns:a="{A}" name="Independent"><a:themeElements><a:clrScheme name="Independent">
<a:dk1><a:sysClr val="windowText" lastClr="112233"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
<a:dk2><a:srgbClr val="222222"/></a:dk2><a:lt2><a:srgbClr val="EEEEEE"/></a:lt2>
<a:accent1><a:srgbClr val="336699"/></a:accent1><a:accent2><a:srgbClr val="CC5500"/></a:accent2>
<a:accent3><a:srgbClr val="55AA55"/></a:accent3><a:accent4><a:srgbClr val="AA55AA"/></a:accent4>
<a:accent5><a:srgbClr val="55AAAA"/></a:accent5><a:accent6><a:srgbClr val="AAAA55"/></a:accent6>
<a:hlink><a:srgbClr val="0000FF"/></a:hlink><a:folHlink><a:srgbClr val="800080"/></a:folHlink>
</a:clrScheme><a:fontScheme name="Independent"><a:majorFont><a:latin typeface="Georgia"/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/></a:minorFont></a:fontScheme><a:fmtScheme name="Independent"/></a:themeElements></a:theme>'''

CHART = f'''<c:chartSpace xmlns:c="{C}" xmlns:a="{A}"><c:chart><c:title><c:tx><c:rich><a:p><a:r><a:t>Regional sales</a:t></a:r></a:p></c:rich></c:tx></c:title><c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:v>Sales</c:v></c:tx><c:spPr><a:solidFill><a:srgbClr val="336699"/></a:solidFill></c:spPr><c:cat><c:strRef><c:f>'Data'!$A$2:$A$4</c:f><c:strCache><c:ptCount val="3"/><c:pt idx="0"><c:v>East</c:v></c:pt><c:pt idx="1"><c:v>West</c:v></c:pt><c:pt idx="2"><c:v>East</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>'Data'!$B$2:$B$4</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="3"/><c:pt idx="0"><c:v>10</c:v></c:pt><c:pt idx="1"><c:v>20</c:v></c:pt><c:pt idx="2"><c:v>5</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:axPos val="b"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:axPos val="l"/><c:crossAx val="1"/></c:valAx></c:plotArea></c:chart></c:chartSpace>'''

def xlsx():
    parts = {
        '[Content_Types].xml': '''<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/xl/charts/chart1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/><Override PartName="/xl/drawings/drawing1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/><Override PartName="/xl/pivotTables/pivotTable1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml"/><Override PartName="/xl/pivotCache/pivotCacheDefinition1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheDefinition+xml"/></Types>''',
        '_rels/.rels': rels(('r1','officeDocument','xl/workbook.xml',False)),
        'xl/workbook.xml': f'''<workbook xmlns="{S}" xmlns:r="{R}"><workbookPr/><sheets><sheet name="Data" sheetId="1" r:id="s1"/><sheet name="Summary" sheetId="2" r:id="s2"/></sheets><definedNames><definedName name="SalesValues">'Data'!$B$2:$B$4</definedName><definedName name="UnsupportedFormula">OFFSET(Data!$A$1,0,0,4,1)</definedName></definedNames><pivotCaches><pivotCache cacheId="0" r:id="pc1"/></pivotCaches></workbook>''',
        'xl/_rels/workbook.xml.rels': rels(('s1','worksheet','worksheets/sheet1.xml',False),('s2','worksheet','worksheets/sheet2.xml',False),('st','styles','styles.xml',False),('th','theme','theme/theme1.xml',False),('pc1','pivotCacheDefinition','pivotCache/pivotCacheDefinition1.xml',False)),
        'xl/styles.xml': f'''<styleSheet xmlns="{S}"><numFmts count="1"><numFmt numFmtId="164" formatCode="0.00%"/></numFmts><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><i/><u/><sz val="16"/><color theme="4"/><name val="Georgia"/></font></fonts><fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FFFFEECC"/></patternFill></fill></fills><borders count="2"><border/><border><left style="thin"><color rgb="FF123456"/></left><bottom style="double"><color theme="4"/></bottom></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="3"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="164" fontId="1" fillId="2" borderId="1" xfId="0" applyAlignment="1"><alignment horizontal="center" vertical="top" wrapText="1"/></xf><xf numFmtId="14" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles><dxfs count="1"><dxf><font><b/><color rgb="FFAA0000"/></font><fill><patternFill patternType="solid"><fgColor rgb="FFFFCCCC"/></patternFill></fill></dxf></dxfs></styleSheet>''',
        'xl/theme/theme1.xml': THEME,
        'xl/worksheets/sheet1.xml': f'''<worksheet xmlns="{S}" xmlns:r="{R}"><dimension ref="A1:D4"/><sheetViews><sheetView workbookViewId="0"><pane xSplit="1" ySplit="1" state="frozen" topLeftCell="B2"/></sheetView></sheetViews><cols><col min="1" max="1" width="20" customWidth="1"/></cols><sheetData><row r="1" ht="30" customHeight="1"><c r="A1" t="inlineStr"><is><t>Region</t></is></c><c r="B1" t="inlineStr"><is><t>Sales</t></is></c><c r="C1"><f>SUM(B2:B4)</f><v>35</v></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>East</t></is></c><c r="B2" s="1"><v>10</v></c><c r="D2" s="2"><v>45292</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>West</t></is></c><c r="B3"><v>20</v></c></row><row r="4"><c r="A4" t="inlineStr"><is><t>East</t></is></c><c r="B4"><v>5</v></c></row></sheetData><conditionalFormatting sqref="B2:B4"><cfRule type="cellIs" operator="greaterThan" dxfId="0" priority="1"><formula>9</formula></cfRule><cfRule type="dataBar" priority="2"><dataBar><cfvo type="min"/><cfvo type="max"/><color rgb="FF00AAFF"/></dataBar></cfRule></conditionalFormatting><dataValidations count="2"><dataValidation type="whole" operator="between" sqref="B2:B4" prompt="Enter sales" error="Out of range"><formula1>0</formula1><formula2>100</formula2></dataValidation><dataValidation type="list" sqref="D3"><formula1>SalesValues</formula1></dataValidation></dataValidations><drawing r:id="dr"/></worksheet>''',
        'xl/worksheets/_rels/sheet1.xml.rels': rels(('dr','drawing','../drawings/drawing1.xml',False)),
        'xl/drawings/drawing1.xml': f'''<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="{A}" xmlns:c="{C}" xmlns:r="{R}"><xdr:twoCellAnchor><xdr:from><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>12</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>16</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame><xdr:nvGraphicFramePr><xdr:cNvPr id="1" name="Chart"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm/><a:graphic><a:graphicData uri="{C}"><c:chart r:id="ch"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>''',
        'xl/drawings/_rels/drawing1.xml.rels': rels(('ch','chart','../charts/chart1.xml',False)),
        'xl/charts/chart1.xml': CHART,
        'xl/worksheets/sheet2.xml': f'''<worksheet xmlns="{S}" xmlns:r="{R}"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Region</t></is></c><c r="B1" t="inlineStr"><is><t>Total Sales</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>East</t></is></c><c r="B2"><v>15</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>West</t></is></c><c r="B3"><v>20</v></c></row></sheetData><pivotTableParts count="1"><pivotTablePart r:id="pt"/></pivotTableParts></worksheet>''',
        'xl/worksheets/_rels/sheet2.xml.rels': rels(('pt','pivotTable','../pivotTables/pivotTable1.xml',False)),
        'xl/pivotTables/pivotTable1.xml': f'''<pivotTableDefinition xmlns="{S}" name="RegionalTotals" cacheId="0" dataCaption="Sales"><location ref="A1:B3" firstHeaderRow="0" firstDataRow="1" firstDataCol="1"/><pivotFields count="2"><pivotField axis="axisRow"/><pivotField dataField="1"/></pivotFields><rowFields count="1"><field x="0"/></rowFields><dataFields count="1"><dataField name="Total Sales" fld="1" subtotal="sum"/></dataFields></pivotTableDefinition>''',
        'xl/pivotCache/pivotCacheDefinition1.xml': f'''<pivotCacheDefinition xmlns="{S}" recordCount="3" refreshOnLoad="1"><cacheSource type="worksheet"><worksheetSource ref="A1:B4" sheet="Data"/></cacheSource><cacheFields count="2"><cacheField name="Region"><sharedItems><s v="East"/><s v="West"/></sharedItems></cacheField><cacheField name="Sales"><sharedItems containsNumber="1"><n v="10"/><n v="20"/><n v="5"/></sharedItems></cacheField></cacheFields></pivotCacheDefinition>''',
    }
    archive('external-rich.xlsx', parts)
    # A normal workbook relationship addresses rich runs in the shared table.
    parts['xl/sharedStrings.xml']=f'<sst xmlns="{S}" count="2" uniqueCount="1"><si><r><rPr><b/></rPr><t>E</t></r><r><rPr><i/></rPr><t>ast</t></r></si></sst>'
    parts['xl/_rels/workbook.xml.rels']=parts['xl/_rels/workbook.xml.rels'].replace('</Relationships>',f'<Relationship Id="sst" Type="{R}/sharedStrings" Target="sharedStrings.xml"/></Relationships>')
    parts['[Content_Types].xml']=parts['[Content_Types].xml'].replace('</Types>','<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>')
    parts['xl/worksheets/sheet1.xml']=parts['xl/worksheets/sheet1.xml'].replace('<c r="A2" t="inlineStr"><is><t>East</t></is></c>','<c r="A2" t="s"><v>0</v></c>').replace('<c r="A4" t="inlineStr"><is><t>East</t></is></c>','<c r="A4" t="s"><v>0</v></c>')
    archive('external-shared-rich.xlsx',parts)


def xfrm(x,y,w,h,rot=0):
    return f'<a:xfrm rot="{rot*60000}"><a:off x="{x*9525}" y="{y*9525}"/><a:ext cx="{w*9525}" cy="{h*9525}"/></a:xfrm>'

def pptx():
    slide = f'''<p:sld xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}" xmlns:c="{C}" xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main"><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="F0F4F8"/></a:solidFill></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr/><p:grpSpPr/>
<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr txBox="1"/><p:nvPr><p:ph type="title" idx="0"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="2400"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mj-lt"/></a:defRPr></a:pPr><a:r><a:rPr b="1"><a:solidFill><a:schemeClr val="accent1"/></a:solidFill><a:hlinkClick r:id="link"/></a:rPr><a:t>Rich </a:t></a:r><a:r><a:rPr i="1" u="sng"/><a:t>title</a:t></a:r></a:p></p:txBody></p:sp>
<p:sp><p:nvSpPr><p:cNvPr id="3" name="Shape"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{xfrm(80,180,240,120,15)}<a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent2"/></a:solidFill><a:ln w="19050"><a:solidFill><a:srgbClr val="123456"/></a:solidFill></a:ln></p:spPr></p:sp>
<p:pic><p:nvPicPr><p:cNvPr id="4" name="Picture" descr="Tiny red pixel"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="img"/><a:srcRect l="10000" t="5000" r="10000" b="0"/></p:blipFill><p:spPr>{xfrm(400,180,200,100)}</p:spPr></p:pic>
<p:grpSp><p:nvGrpSpPr><p:cNvPr id="5" name="Group"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="762000" y="3810000"/><a:ext cx="3810000" cy="1905000"/><a:chOff x="0" y="0"/><a:chExt cx="1905000" cy="952500"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="6" name="Grouped ellipse"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{xfrm(10,20,50,30)}<a:prstGeom prst="ellipse"/><a:solidFill><a:srgbClr val="55AA55"/></a:solidFill></p:spPr></p:sp></p:grpSp>
<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="7" name="Chart"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="6667500" y="1714500"/><a:ext cx="4762500" cy="2857500"/></p:xfrm><a:graphic><a:graphicData uri="{C}"><c:chart r:id="chart"/></a:graphicData></a:graphic></p:graphicFrame>
<p:pic><p:nvPicPr><p:cNvPr id="8" name="External image"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:link="outside"/></p:blipFill><p:spPr>{xfrm(1000,550,100,100)}</p:spPr></p:pic>
<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="9" name="Unsupported SmartArt"/></p:nvGraphicFramePr><p:xfrm/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:graphic></p:graphicFrame>
</p:spTree></p:cSld><p:transition p14:dur="750"><p:fade/></p:transition><p:timing><p:tnLst><p:par><p:cTn id="1" nodeType="tmRoot"><p:childTnLst><p:par><p:cTn id="2" presetID="10" nodeType="withEffect"><p:stCondLst><p:cond delay="120"/></p:stCondLst><p:childTnLst><p:animEffect transition="in" filter="fade"><p:cBhvr><p:cTn id="3" dur="600"/><p:tgtEl><p:spTgt spid="3"/></p:tgtEl></p:cBhvr></p:animEffect></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:tnLst></p:timing></p:sld>'''
    parts = {
        '[Content_Types].xml': '''<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/ppt/charts/chart1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/><Override PartName="/ppt/notesSlides/notesSlide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml"/></Types>''',
        '_rels/.rels': rels(('r1','officeDocument','ppt/presentation.xml',False)),
        'ppt/presentation.xml': f'<p:presentation xmlns:p="{P}" xmlns:r="{R}"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="master"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="slide"/></p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>',
        'ppt/_rels/presentation.xml.rels': rels(('slide','slide','slides/slide1.xml',False),('master','slideMaster','slideMasters/slideMaster1.xml',False)),
        'ppt/slides/slide1.xml': slide,
        'ppt/slides/_rels/slide1.xml.rels': rels(('layout','slideLayout','../slideLayouts/slideLayout1.xml',False),('img','image','../media/pixel.png',False),('chart','chart','../charts/chart1.xml',False),('notes','notesSlide','../notesSlides/notesSlide1.xml',False),('link','hyperlink','https://example.org/safe',True),('outside','image','https://127.0.0.1:9/do-not-fetch.png',True)),
        'ppt/slideLayouts/slideLayout1.xml': f'<p:sldLayout xmlns:p="{P}" xmlns:a="{A}"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2"/><p:cNvSpPr txBox="1"/><p:nvPr><p:ph type="title" idx="0"/></p:nvPr></p:nvSpPr><p:spPr>{xfrm(80,60,1100,80)}</p:spPr></p:sp></p:spTree></p:cSld></p:sldLayout>',
        'ppt/slideLayouts/_rels/slideLayout1.xml.rels': rels(('master','slideMaster','../slideMasters/slideMaster1.xml',False)),
        'ppt/slideMasters/slideMaster1.xml': f'<p:sldMaster xmlns:p="{P}" xmlns:a="{A}"><p:cSld><p:spTree/></p:cSld></p:sldMaster>',
        'ppt/slideMasters/_rels/slideMaster1.xml.rels': rels(('theme','theme','../theme/theme1.xml',False)),
        'ppt/theme/theme1.xml': THEME,
        'ppt/media/pixel.png': base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=='),
        'ppt/charts/chart1.xml': CHART,
        'ppt/notesSlides/notesSlide1.xml': f'<p:notes xmlns:p="{P}" xmlns:a="{A}"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Presenter notes</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>',
    }
    archive('external-rich.pptx',parts)

def pptx_reordered():
    """Put an independently authored plain slide before the rich slide1."""
    with ZipFile(HERE / 'external-rich.pptx') as source:
        parts = {name: source.read(name) for name in source.namelist()}
    assert not any(name.startswith('customXml/') for name in parts)
    edits = {
        'ppt/presentation.xml': ('<p:sldIdLst>', '<p:sldIdLst><p:sldId id="257" r:id="leading"/>'),
        'ppt/_rels/presentation.xml.rels': ('</Relationships>', f'<Relationship Id="leading" Type="{R}/slide" Target="slides/slide2.xml"/></Relationships>'),
        '[Content_Types].xml': ('</Types>', '<Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>'),
    }
    for name, (needle, replacement) in edits.items():
        xml = parts[name].decode('utf-8')
        assert xml.count(needle) == 1
        parts[name] = xml.replace(needle, replacement).encode('utf-8')
    with ZipFile(HERE / 'external-slide-order.pptx') as source:
        parts['ppt/slides/slide2.xml'] = source.read('ppt/slides/slide2.xml')
    # Fixed ZIP headers make this combined fixture reproducible byte for byte.
    with ZipFile(HERE / 'external-rich-reordered.pptx', 'w') as output:
        for name, data in sorted(parts.items()):
            entry = ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            entry.compress_type = ZIP_DEFLATED
            output.writestr(entry, data)


if __name__ == '__main__':
    xlsx()
    pptx()
    pptx_reordered()
