#!/usr/bin/env python3
"""Content diff of an office file before and after a save: cells (xlsx), slide text (pptx), body text (docx).
Prints counts and same/different only, never document text.

  celldiff.py ORIGINAL SAVED     one report
  celldiff.py --dir DIR          pairs from `corpus-roundtrip.sh`
"""
import re, sys, zipfile, hashlib, pathlib

def texts(z, names, tag):
    out = []
    for n in names:
        x = z.read(n).decode("utf-8", "replace")
        out += re.findall(rf"<{tag}(?:\s[^>]*)?>(.*?)</{tag}>", x, flags=re.S)
    return out

def attr(tag, name):
    m = re.search(rf'\b{name}="([^"]*)"', tag); return m.group(1) if m else None

def content(path):
    z = zipfile.ZipFile(path); names = z.namelist()
    if path.endswith(".docx"):
        return "body text", texts(z, [n for n in names if re.match(r"word/document\d*\.xml$", n)], "w:t"), 0
    if path.endswith(".pptx"):
        sl = sorted((n for n in names if re.match(r"ppt/slides/slide\d+\.xml$", n)), key=lambda n: int(re.findall(r"\d+", n)[0]))
        return "slide text", texts(z, sl, "a:t"), 0
    ss = []
    if "xl/sharedStrings.xml" in names:
        ss = ["".join(re.findall(r"<t(?:\s[^>]*)?>(.*?)</t>", si, flags=re.S)) for si in z.read("xl/sharedStrings.xml").decode("utf-8", "replace").split("<si>")[1:]]
    wb = z.read("xl/workbook.xml").decode("utf-8", "replace")
    rels = dict(re.findall(r'<Relationship [^>]*?Id="([^"]+)"[^>]*?Target="([^"]+)"', z.read("xl/_rels/workbook.xml.rels").decode("utf-8", "replace")))
    cells, formulas = [], 0
    for s in wb.split("<sheet ")[1:]:
        t = rels.get(attr(s, "r:id") or "")
        if not t: continue
        t = "xl/" + t.lstrip("/").removeprefix("xl/")
        if t not in names: continue
        for c in z.read(t).decode("utf-8", "replace").split("<c ")[1:]:
            head = c.split(">")[0]; body = c.split("</c>")[0]
            f = "<f" in body; formulas += f
            v = re.search(r"<v[^>]*>(.*?)</v>", body, flags=re.S)
            if v: val = ss[int(v.group(1))] if attr(head, "t") == "s" and v.group(1).isdigit() and int(v.group(1)) < len(ss) else v.group(1)
            else:
                i = re.findall(r"<t[^>]*>(.*?)</t>", body, flags=re.S); val = "".join(i) if i else None
            if val is not None: cells.append((attr(head, "r"), "" if f else val))
    return "cells", cells, formulas

def digest(items): return hashlib.sha256(repr(items).encode()).hexdigest()[:8]

def report(name, a, b):
    ka, ia, fa = content(a); kb, ib, fb = content(b)
    same = ia == ib
    print(f"{name}: {ka} {len(ia)}->{len(ib)} same={same} formulas {fa}->{fb}" + ("" if same else f" ({digest(ia)} vs {digest(ib)})"))

if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] != "--dir":
        report(sys.argv[2], sys.argv[1], sys.argv[2])
    elif len(sys.argv) == 3:
        d = pathlib.Path(sys.argv[2]); corpus = pathlib.Path(__file__).resolve().parent.parent / "corpus"
        for s in sorted((d / "saved").iterdir()):
            o = corpus / s.name.replace("__", "/"); report(str(o.relative_to(corpus)), str(o), str(s))
    else:
        sys.exit(__doc__)
