"""Read Contrix's standard Office output with independent Python readers.

Requires openpyxl and python-pptx (known-good local versions: 3.1.5 / 1.0.2).
First run the office_fixture_smoke Rust example with a workspace output folder.
Usage: python crates/export/tests/verify_office_readers.py <output-dir>
Neither reader consults Contrix customXml. Tests assert standard parts' content.
"""
import sys
import json
from pathlib import Path
from zipfile import ZipFile
from xml.etree import ElementTree as E
import openpyxl
from pptx import Presentation

folder=Path(sys.argv[1])
workbook=openpyxl.load_workbook(folder/'imported-standard.xlsx')
cell=workbook['Data']['B2']
assert cell.value==10 and cell.font.name=='Georgia' and cell.font.bold
assert cell.font.color.rgb=='FF336699' and cell.fill.fgColor.rgb=='FFFFEECC'
assert cell.border.bottom.style=='double' and cell.alignment.wrap_text
assert cell.number_format=='0.00%'
assert cell.border.left.color.rgb=='FF123456'
assert cell.alignment.horizontal=='center' and cell.alignment.vertical=='top'
print('Independent XLSX styles: ' + json.dumps({
    'number_format': cell.number_format, 'font': cell.font.name,
    'font_color': cell.font.color.rgb, 'fill_color': cell.fill.fgColor.rgb,
    'left_border_color': cell.border.left.color.rgb, 'bottom_border': cell.border.bottom.style,
    'horizontal_alignment': cell.alignment.horizontal, 'vertical_alignment': cell.alignment.vertical,
    'wrap_text': cell.alignment.wrap_text,
}, sort_keys=True))
assert 'SalesValues' in workbook.defined_names
assert len(workbook['Data'].data_validations.dataValidation)==1
assert len(workbook['Data']._charts)==1 and len(workbook['Summary']._pivots)==1
assert workbook['Summary']._pivots[0].cache.recordCount==3
presentation=Presentation(folder/'imported-standard.pptx')
assert (presentation.slide_width,presentation.slide_height)==(12192000,6858000)
slide=presentation.slides[0]
assert len(slide.shapes)==5
title=slide.shapes[0]
assert title.left==762000 and title.top==571500
assert title.text=='Rich title'
run=title.text_frame.paragraphs[0].runs[0]
assert run.font.name=='Georgia' and run.font.bold and str(run.font.color.rgb)=='336699'
assert run.hyperlink.address=='https://example.org/safe'
assert slide.shapes[1].rotation==15 and slide.shapes[2].crop_left==0.1
assert slide.shapes[4].has_chart and tuple(slide.shapes[4].chart.series[0].values)==(10.,20.,5.)
assert slide.notes_slide.notes_text_frame.text=='Presenter notes'
with ZipFile(folder/'imported-standard.pptx') as package:
    root=E.fromstring(package.read('ppt/slides/slide1.xml'))
    ns={'p':'http://schemas.openxmlformats.org/presentationml/2006/main'}
    transition=root.find('p:transition',ns)
    assert transition.get('{http://schemas.microsoft.com/office/powerpoint/2010/main}dur')=='750'
    assert root.find('.//p:animEffect',ns).get('filter')=='fade'
print(f'Independent standard-part readers passed: openpyxl {openpyxl.__version__}; python-pptx {sys.modules["pptx"].__version__}')
