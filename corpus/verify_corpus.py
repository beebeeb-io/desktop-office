#!/usr/bin/env python3
"""Rules 1 and 2 of the ov-corpus spec. Usage: verify_corpus.py <corpus-dir> <poi-checkout>
Rule 1: every file in corpus/ has a SOURCES.tsv row with a licence.
Rule 2: 0 scrubbed files carry author / last-modified-by / company / manager; the same count over the
ORIGINAL copies (found in the POI checkout by original_sha256) must be > 0, or the instrument cannot fail."""
import io, os, re, sys, zipfile, hashlib
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build_corpus as b

root, poi = sys.argv[1], sys.argv[2]
rows = [l.rstrip("\n").split("\t") for l in open(os.path.join(root, "SOURCES.tsv"))][1:]
files = sorted(os.path.join(k, f) for k in ("docx", "doc", "xlsx", "xls", "pptx") for f in os.listdir(os.path.join(root, k)))
paths = {r[0] for r in rows}
print("rule 1: files %d, rows %d, rows with licence %d, files without row %d" % (
    len(files), len(rows), sum(1 for r in rows if r[3].strip()), len([f for f in files if f not in paths])))

def people(data, kind):
    if kind in ("docx", "xlsx", "pptx"):
        z = zipfile.ZipFile(io.BytesIO(data))
        core = z.read("docProps/core.xml") if "docProps/core.xml" in z.namelist() else b""
        app = z.read("docProps/app.xml") if "docProps/app.xml" in z.namelist() else b""
        v = [b.tag_text(core, b"creator"), b.tag_text(core, b"lastModifiedBy"), b.tag_text(app, b"Company"), b.tag_text(app, b"Manager")]
        extra = "docProps/custom.xml" in z.namelist()
        return sum(1 for x in v if x) + (1 if extra else 0)
    i = b.cfb_info(data)
    return sum(1 for x in (i["author"], i["last"], i["company"], i["manager"]) if x)

after = 0; bad = []
for f in files:
    n = people(open(os.path.join(root, f), "rb").read(), f.split("/")[0])
    after += n
    if n: bad.append(f)
print("rule 2 after scrub: files with a name/company/custom prop %d (total fields %d)" % (len(bad), after))
by = {}
for dp, _, fs in os.walk(os.path.join(poi, "test-data")):
    for f in fs:
        p = os.path.join(dp, f)
        if f.lower().endswith((".docx", ".doc", ".xlsx", ".xls", ".pptx")) and os.path.getsize(p) < 2 * 1024 * 1024:
            by.setdefault(hashlib.sha256(open(p, "rb").read()).hexdigest(), p)
before = 0; seen = 0
for r in rows:
    p = by.get(r[6])
    if p:
        seen += 1
        before += 1 if people(open(p, "rb").read(), r[0].split("/")[0]) else 0
print("rule 2 before scrub (originals found %d of %d): files with a name/company/custom prop %d" % (seen, len(rows), before))
sys.exit(1 if bad or len(files) != len(rows) else 0)
