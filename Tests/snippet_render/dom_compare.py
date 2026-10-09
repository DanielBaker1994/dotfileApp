"""Compare the reading view's HTML (gfm+sourcepos) with Export PDF's (gfm)
the way a browser sees them: HTML5 tree building (tinyhtml5, weasyprint's own
parser), sourcepos wrappers (data-pos / data-wrapper) unwrapped. Also flags
markdown syntax that leaked into the rendered text.
Usage: python dom_compare.py DIR   (DIR holds NAME.pdf.html + NAME.scr.html)
"""
import glob, os, re, sys
import tinyhtml5

def canon(el, out):
    tag = el.tag.split('}')[-1]
    attrs = {k: v for k, v in el.attrib.items() if k not in ('data-pos', 'data-wrapper')}
    bare = tag in ('span', 'div') and not attrs and ('data-wrapper' in el.attrib or 'data-pos' in el.attrib)
    if not bare:
        out.append('<%s%s>' % (tag, ''.join(' %s="%s"' % kv for kv in sorted(attrs.items()))))
    out.append(el.text or '')
    for c in el:
        canon(c, out)
    if not bare:
        out.append('</%s>' % tag)
    out.append(el.tail or '')

def body(path):
    doc = tinyhtml5.parse(open(path, encoding='utf-8').read())
    b = next(e for e in doc.iter() if e.tag.split('}')[-1] == 'body')
    out = []
    canon(b, out)
    return re.sub(r'\s+', ' ', ''.join(out)).replace('> <', '><').strip()

# placeholders: ${1:…} / $1 left unexpanded (a "$12k" in a table is text)
LEAKS = [(r'☐|☒', 'task box glyph'), (r'\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]', 'alert marker'),
         (r'\$\{\d+[:}]|\$\d+(?![\w.,])', 'snippet placeholder'), (r'```', 'code fence'), (r'\|\s*-{3}', 'table rule'),
         (r'&lt;/?(span|div|kbd|details|summary)\b', 'escaped HTML'), (r'\*\*\S', 'bold marker')]

bad = 0
files = sorted(glob.glob(os.path.join(sys.argv[1], '*.scr.html')))
for scr in files:
    name = os.path.basename(scr)[:-len('.scr.html')]
    a, b = body(scr[:-len('.scr.html')] + '.pdf.html'), body(scr)
    if a != b:
        i = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
        print('  FAIL %s: reading view ≠ PDF\n    pdf: …%s\n    scr: …%s' % (name, a[max(0, i - 60):i + 100], b[max(0, i - 60):i + 100]))
        bad += 1
    text = re.sub(r'<(pre|svg|code|img)\b.*?</\1>|<img[^>]*>', '', b, flags=re.S)
    for pat, why in LEAKS:
        if re.search(pat, text):
            print('  FAIL %s: %s left in the rendered text' % (name, why))
            bad += 1
print('snippet render: %d snippets, %d failures' % (len(files), bad))
sys.exit(1 if bad or not files else 0)
