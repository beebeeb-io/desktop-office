#!/usr/bin/env python3
"""Build corpus/ from a checkout of Apache POI's test-data (Apache-2.0).

Usage:  python3 -I corpus/build_corpus.py <poi-checkout> <corpus-dir> [--per-type N]

For each type it keeps files that (a) open as a zip / compound file, (b) are not password
protected, (c) carry no tracked-change / comment authors and no absolute local path, (d) can be
scrubbed of author / last-modified-by / company / manager / revision / custom properties without
touching any content part (checked by hashing the content parts before and after). Then it writes
the scrubbed copies, SOURCES.tsv and EXPECTED.tsv. Deterministic: candidates are ordered by the
sha256 of the original bytes.
"""
import hashlib, io, os, re, struct, sys, zipfile, shutil

try:
    import olefile
except ImportError:
    sys.exit("needs olefile: pip install --user olefile")

POI_DIRS = {
    "docx": "test-data/document", "doc": "test-data/document",
    "xlsx": "test-data/spreadsheet", "xls": "test-data/spreadsheet",
    "pptx": "test-data/slideshow",
}
BAD_NAME = re.compile(r"fuzz|corrupt|invalid|truncat|broken|bad|encrypt|protect|passw|empty|crash|malformed|zero|"
                      r"clusterfuzz|poc|leak|ooxml-strict|template", re.I)
MAX_BYTES = 2 * 1024 * 1024
ABS_PATH = re.compile(rb"[A-Za-z]:\\\\?(Users|Documents and Settings|Dokumente)|/Users/|/home/|\\\\[A-Za-z0-9_.-]+\\[A-Za-z0-9$_.-]+", re.I)
META_PARTS = {"[Content_Types].xml", "_rels/.rels"}


def sha(b): return hashlib.sha256(b).hexdigest()


def is_meta(name): return name in META_PARTS or name.startswith("docProps/")


# ---------- OOXML ----------
def tag_re(local):
    return re.compile(rb"<(?:\w+:)?%s(?:\s[^>]*)?(?:/>|>.*?</(?:\w+:)?%s>)" % (local, local), re.S)


def tag_text(xml, local):
    m = re.search(rb"<(?:\w+:)?%s(?:\s[^>]*)?>(.*?)</(?:\w+:)?%s>" % (local, local), xml, re.S)
    return m.group(1).decode("utf8", "replace").strip() if m else ""


def content_hash(zf, names):
    h = hashlib.sha256()
    for n in sorted(names):
        h.update(n.encode()); h.update(b"\0"); h.update(sha(zf.read(n)).encode()); h.update(b"\n")
    return h.hexdigest()


def ooxml_inspect(data, kind):
    """Return dict or None (None => unusable). Does not modify."""
    try:
        zf = zipfile.ZipFile(io.BytesIO(data))
        names = zf.namelist()
    except Exception:
        return None
    if "EncryptedPackage" in names or any(n.lower().startswith("encrypt") for n in names):
        return None
    if zf.testzip() is not None:
        return None
    # tracked-change / comment authors live in content parts: leave such files out
    for n in names:
        low = n.lower()
        if re.search(r"comments|people\.xml|commentauthors|persons/|threadedcomments", low):
            return None
        if low.endswith(".xml") or low.endswith(".rels"):
            raw = zf.read(n)
            if re.search(rb"<w:(ins|del|moveFrom|moveTo)\s[^>]*w:author=", raw):
                return None
            if ABS_PATH.search(raw):
                return None
    core = zf.read("docProps/core.xml") if "docProps/core.xml" in names else b""
    app = zf.read("docProps/app.xml") if "docProps/app.xml" in names else b""
    if not app:
        return None
    people = [tag_text(core, b"creator"), tag_text(core, b"lastModifiedBy"),
              tag_text(app, b"Company"), tag_text(app, b"Manager")]
    people = [p for p in people if len(p) >= 3]
    # the same names elsewhere in a content part => cannot scrub without changing content
    for n in names:
        if is_meta(n):
            continue
        raw = zf.read(n)
        for p in people:
            if p.encode("utf8") in raw or p.encode("utf-16le") in raw:
                return None
    appname = tag_text(app, b"Application")
    appver = tag_text(app, b"AppVersion")
    exp = "unknown"
    if kind == "docx":
        v = tag_text(app, b"Pages")
        exp = v if v.isdigit() and int(v) > 0 else "unknown"
    elif kind == "pptx":
        v = tag_text(app, b"Slides")
        exp = v if v.isdigit() and int(v) > 0 else "unknown"
    elif kind == "xlsx":
        try:
            wb = zf.read("xl/workbook.xml")
            n = len(re.findall(rb"<(?:\w+:)?sheet\s", wb))
            exp = str(n) if n else "unknown"
        except KeyError:
            return None
    return {"producer": (appname + (" " + appver if appver else "")).strip() or "unknown", "expected": exp}


def ooxml_scrub(data):
    zin = zipfile.ZipFile(io.BytesIO(data))
    names = zin.namelist()
    content = [n for n in names if not is_meta(n)]
    before = content_hash(zin, content)
    out = io.BytesIO()
    removed = []
    with zipfile.ZipFile(out, "w") as zout:
        for info in zin.infolist():
            n = info.filename
            raw = zin.read(n)
            if n == "docProps/custom.xml":
                removed.append("custom"); continue
            if n == "docProps/core.xml":
                for t in (b"creator", b"lastModifiedBy", b"revision", b"contributor", b"manager"):
                    raw = tag_re(t).sub(b"", raw)
                removed.append("core")
            elif n == "docProps/app.xml":
                for t in (b"Company", b"Manager", b"HyperlinkBase", b"HLinks"):
                    raw = tag_re(t).sub(b"", raw)
                removed.append("app")
            elif n == "_rels/.rels":
                raw = re.sub(rb"<Relationship\s[^>]*custom-properties[^>]*/>", b"", raw)
            elif n == "[Content_Types].xml":
                raw = re.sub(rb"<Override\s[^>]*PartName=\"/docProps/custom\.xml\"[^>]*/>", b"", raw)
            zi = zipfile.ZipInfo(n, date_time=(1980, 1, 1, 0, 0, 0))
            zi.compress_type = info.compress_type
            zi.external_attr = info.external_attr
            zout.writestr(zi, raw)
    new = out.getvalue()
    zchk = zipfile.ZipFile(io.BytesIO(new))
    after = content_hash(zchk, [n for n in zchk.namelist() if not is_meta(n)])
    return new, before, after, ",".join(removed)


# ---------- CFB (.doc / .xls) ----------
SI = "\x05SummaryInformation"
DSI = "\x05DocumentSummaryInformation"


def parse_props(stream):
    """yield (set_index, pid, type, value_offset, value_bytes_len) for string/int props."""
    res = []
    if len(stream) < 48:
        return res
    nsets = struct.unpack_from("<I", stream, 24)[0]
    for s in range(min(nsets, 2)):
        off = struct.unpack_from("<I", stream, 28 + s * 20 + 16)[0]
        if off + 8 > len(stream):
            continue
        nprops = struct.unpack_from("<I", stream, off + 4)[0]
        for i in range(min(nprops, 200)):
            pid, poff = struct.unpack_from("<II", stream, off + 8 + i * 8)
            vo = off + poff
            if vo + 4 > len(stream):
                continue
            t = struct.unpack_from("<I", stream, vo)[0] & 0xFFFF
            if t in (0x1E, 0x1F) and vo + 8 <= len(stream):
                ln = struct.unpack_from("<I", stream, vo + 4)[0]
                bl = ln * (2 if t == 0x1F else 1)
                res.append((s, pid, t, vo + 8, bl))
            elif t == 0x03 and vo + 8 <= len(stream):
                res.append((s, pid, t, vo + 4, 4))
    return res


def prop_str(stream, p):
    _, _, t, o, bl = p
    raw = stream[o:o + bl]
    return (raw.decode("utf-16le", "replace") if t == 0x1F else raw.decode("cp1252", "replace")).strip("\0 ").strip()


def cfb_info(path_or_bytes):
    ole = olefile.OleFileIO(io.BytesIO(path_or_bytes) if isinstance(path_or_bytes, bytes) else path_or_bytes)
    info = {"author": "", "last": "", "app": "", "pages": "", "company": "", "manager": "", "streams": ole.listdir(streams=True, storages=False)}
    if ole.exists(SI):
        s = ole.openstream(SI).read()
        for p in parse_props(s):
            if p[0] == 0 and p[1] == 4 and p[2] in (0x1E, 0x1F): info["author"] = prop_str(s, p)
            if p[0] == 0 and p[1] == 8 and p[2] in (0x1E, 0x1F): info["last"] = prop_str(s, p)
            if p[0] == 0 and p[1] == 18 and p[2] in (0x1E, 0x1F): info["app"] = prop_str(s, p)
            if p[0] == 0 and p[1] == 14 and p[2] == 0x03: info["pages"] = str(struct.unpack_from("<i", s, p[3])[0])
    if ole.exists(DSI):
        s = ole.openstream(DSI).read()
        for p in parse_props(s):
            if p[0] == 0 and p[1] == 15 and p[2] in (0x1E, 0x1F): info["company"] = prop_str(s, p)
            if p[0] == 0 and p[1] == 14 and p[2] in (0x1E, 0x1F): info["manager"] = prop_str(s, p)
    ole.close()
    return info


def cfb_content_hash(data):
    ole = olefile.OleFileIO(io.BytesIO(data))
    h = hashlib.sha256()
    for st in sorted(ole.listdir(streams=True, storages=False)):
        n = "/".join(st)
        if n in (SI, DSI):
            continue
        h.update(n.encode()); h.update(b"\0"); h.update(sha(ole.openstream(st).read()).encode()); h.update(b"\n")
    ole.close()
    return h.hexdigest()


def xls_sheet_count(data):
    """Count BOUNDSHEET records (0x0085) in the workbook-globals substream of a BIFF8 Workbook stream."""
    ole = olefile.OleFileIO(io.BytesIO(data))
    name = "Workbook" if ole.exists("Workbook") else "Book"
    st = ole.openstream(name).read()
    ole.close()
    pos, n = 0, 0
    while pos + 4 <= len(st):
        rid, ln = struct.unpack_from("<HH", st, pos)
        if rid == 0x0085:
            n += 1
        if rid == 0x000A:  # EOF of the globals substream
            break
        pos += 4 + ln
    return n


def cfb_inspect(data, kind):
    try:
        if not olefile.isOleFile(data):
            return None
        ole = olefile.OleFileIO(io.BytesIO(data))
        names = ["/".join(s) for s in ole.listdir(streams=True, storages=False)]
        ole.close()
    except Exception:
        return None
    if "EncryptedPackage" in names or "EncryptionInfo" in names:
        return None
    if kind == "doc" and "WordDocument" not in names:
        return None
    if kind == "xls" and not ("Workbook" in names or "Book" in names):
        return None
    if ABS_PATH.search(data):
        return None
    info = cfb_info(data)
    people = [x for x in (info["author"], info["last"], info["company"], info["manager"]) if len(x) >= 3]
    # the name must not occur in a content stream (revision author tables, etc.)
    ole = olefile.OleFileIO(io.BytesIO(data))
    for st in ole.listdir(streams=True, storages=False):
        n = "/".join(st)
        if n in (SI, DSI):
            continue
        raw = ole.openstream(st).read()
        for p in people:
            if p.encode("cp1252", "ignore") in raw or p.encode("utf-16le") in raw:
                ole.close(); return None
    ole.close()
    exp = info["pages"] if kind == "doc" and info["pages"].isdigit() and int(info["pages"]) > 0 else "unknown"
    if kind == "xls":
        c = xls_sheet_count(data)
        exp = str(c) if c else "unknown"
    return {"producer": info["app"] or "unknown", "expected": exp}


def cfb_scrub(data):
    before = cfb_content_hash(data)
    buf = bytearray(data)
    tmp = io.BytesIO(bytes(buf))
    ole = olefile.OleFileIO(tmp, write_mode=True)
    removed = []
    for sname, targets in ((SI, {(0, 4), (0, 8), (0, 12), (0, 6), (0, 2), (0, 5)}), (DSI, {(0, 15), (0, 14)})):
        if not ole.exists(sname):
            continue
        s = bytearray(ole.openstream(sname).read())
        for p in parse_props(bytes(s)):
            if (p[0], p[1]) in targets and p[2] in (0x1E, 0x1F):
                # 2=title 4=author 5=keywords 6=comments 8=last author 12=create time(not str) -> blank the string bytes
                s[p[3]:p[3] + p[4]] = b"\0" * p[4]
                removed.append("%s:%d" % ("si" if sname == SI else "dsi", p[1]))
        ole.write_stream(sname, bytes(s))
    ole.close()
    new = tmp.getvalue()
    after = cfb_content_hash(new)
    return new, before, after, ",".join(sorted(set(removed)))


def main():
    poi, out_dir = sys.argv[1], sys.argv[2]
    per = int(sys.argv[sys.argv.index("--per-type") + 1]) if "--per-type" in sys.argv else 24
    rev = os.popen("git -C %s rev-parse HEAD" % poi).read().strip()
    rows, expected = [], []
    for kind, sub in POI_DIRS.items():
        d = os.path.join(poi, sub)
        cands = []
        for f in sorted(os.listdir(d)):
            if not f.lower().endswith("." + kind) or BAD_NAME.search(f):
                continue
            p = os.path.join(d, f)
            if not os.path.isfile(p) or os.path.getsize(p) > MAX_BYTES or os.path.getsize(p) < 2000:
                continue
            data = open(p, "rb").read()
            cands.append((sha(data), f, data))
        cands.sort()
        kept = 0
        tdir = os.path.join(out_dir, kind)
        shutil.rmtree(tdir, ignore_errors=True); os.makedirs(tdir)
        # first pass: Microsoft-produced files, second pass: the rest, so a type short of 20 says so
        for want_ms in (True, False):
            for h, f, data in cands:
                if kept >= per:
                    break
                if os.path.exists(os.path.join(tdir, f)):
                    continue
                try:
                    ins = (ooxml_inspect if kind in ("docx", "xlsx", "pptx") else cfb_inspect)(data, kind)
                    if ins is None:
                        continue
                    if want_ms != ("microsoft" in ins["producer"].lower()):
                        continue
                    new, b, a, removed = (ooxml_scrub if kind in ("docx", "xlsx", "pptx") else cfb_scrub)(data)
                except Exception as e:  # unreadable => left out
                    continue
                if b != a:
                    continue
                open(os.path.join(tdir, f), "wb").write(new)
                rows.append(("%s/%s" % (kind, f), sha(new), "https://github.com/apache/poi/blob/%s/%s/%s" % (rev, sub, f),
                             "Apache-2.0 (Apache POI, legal/LICENSE)", ins["producer"], "yes: %s; content parts %s before=after" % (removed or "none", b[:16]), sha(data)))
                expected.append(("%s/%s" % (kind, f), ins["expected"]))
                kept += 1
        print(kind, kept, file=sys.stderr)
    rows.sort(); expected.sort()
    with open(os.path.join(out_dir, "SOURCES.tsv"), "w") as fh:
        fh.write("path\tsha256\torigin_url\tlicence\tproducer\tscrubbed\toriginal_sha256\n")
        for r in rows: fh.write("\t".join(r) + "\n")
    with open(os.path.join(out_dir, "EXPECTED.tsv"), "w") as fh:
        fh.write("path\texpected_count\n")
        for r in expected: fh.write("\t".join(r) + "\n")


if __name__ == "__main__":
    main()
