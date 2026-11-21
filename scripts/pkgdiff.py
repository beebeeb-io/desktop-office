#!/usr/bin/env python3
"""Package diff of an office file before and after a save: parts lost, added, changed, and the watched
features that went missing. Prints part names (digits folded) and counts, never document text.

  pkgdiff.py ORIGINAL SAVED     one report
  pkgdiff.py --dir DIR          pairs from `corpus-roundtrip.sh` (DIR/saved/<path with / as __>)
"""
import re, sys, zipfile, collections, pathlib

def cls(n): return re.sub(r"\d+", "N", n)

def parts(path):
    with zipfile.ZipFile(path) as z:
        return {i.filename: z.read(i) for i in z.infolist() if not i.is_dir()}

def norm(name, b):
    if not name.endswith((".xml", ".rels", ".vml")): return b
    s = b.decode("utf-8", "replace").lstrip("﻿")
    s = re.sub(r"^<\?xml.*?\?>", "", s, flags=re.S)
    return re.sub(r">\s+<", "><", s).strip().encode()

def count(p, pred, needle):
    return sum(b.decode("utf-8", "replace").count(needle) for n, b in p.items() if pred(n))

def features(p):
    has = lambda f: sum(1 for n in p if f(n))
    ext_img = 0
    for n, b in p.items():
        if n.endswith(".rels"):
            ext_img += sum(1 for r in b.decode("utf-8", "replace").split("<Relationship ") if "/image" in r and 'TargetMode="External"' in r)
    word = lambda n: n.startswith("word/") and n.endswith(".xml")
    return {
        "macros": has(lambda n: n.endswith(("vbaProject.bin", "vbaData.xml"))),
        "transitions": count(p, lambda n: n.startswith("ppt/slides/slide"), "<p:transition"),
        "linked_images": ext_img,
        "chart_style_parts": has(lambda n: "/charts/style" in n or "/charts/colors" in n),
        "comments": has(lambda n: ("comments" in n and n.endswith(".xml")) or "commentAuthors" in n or "threadedComments" in n),
        "tracked_changes": count(p, word, "<w:ins ") + count(p, word, "<w:del "),
        "pivots": has(lambda n: "pivotTable" in n or "pivotCache" in n),
        "smartart": has(lambda n: "/diagrams/" in n),
        "equations": count(p, lambda n: n.endswith(".xml"), "<m:oMath"),
        "ole_objects": has(lambda n: "/embeddings/" in n) + count(p, lambda n: n.endswith(".xml"), "<o:OLEObject") + count(p, lambda n: n.endswith(".xml"), "<p:oleObj"),
        "signatures": has(lambda n: n.startswith("_xmlsignatures/")),
    }

def diff(a, b):
    A, B = parts(a), parts(b)
    lost = sorted(set(A) - set(B)); added = sorted(set(B) - set(A))
    changed = sorted(n for n in A if n in B and norm(n, A[n]) != norm(n, B[n]))
    ca = collections.Counter(map(cls, A)); cb = collections.Counter(map(cls, B))
    deficits = {k: n - cb.get(k, 0) for k, n in ca.items() if cb.get(k, 0) < n}
    fa, fb = features(A), features(B)
    floss = {k: (n, fb[k]) for k, n in fa.items() if fb[k] < n}
    return dict(lost=lost, added=added, changed=changed, deficits=deficits, feature_loss=floss, parts=(len(A), len(B)))

def show(name, d):
    print(f"{name}: parts {d['parts'][0]}->{d['parts'][1]} lost={len(d['lost'])} added={len(d['added'])} changed={len(d['changed'])}")
    for k, n in sorted(d["deficits"].items()): print(f"  lost class {k} x{n}")
    for k, (x, y) in sorted(d["feature_loss"].items()): print(f"  feature {k}: {x}->{y}")

if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] != "--dir":
        show(sys.argv[2], diff(sys.argv[1], sys.argv[2]))
    elif len(sys.argv) == 3:
        d = pathlib.Path(sys.argv[2]); corpus = pathlib.Path(__file__).resolve().parent.parent / "corpus"
        tot = collections.Counter()
        for s in sorted((d / "saved").iterdir()):
            o = corpus / s.name.replace("__", "/")
            r = diff(o, s); show(o.relative_to(corpus), r)
            for k in r["deficits"]: tot[k] += r["deficits"][k]
        print("TOTAL lost part classes:")
        for k, n in tot.most_common(): print(f"  {k} x{n}")
    else:
        sys.exit(__doc__)
