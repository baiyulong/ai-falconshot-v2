#!/usr/bin/env python3
# P9 part 2: the non-Rust half of the license question.
#  - does this Qt install contain any license TEXT we can ship?
#  - what binaries does the pinned package actually redistribute?
#  - is the MSVC redistributable present locally?
#  - how many distinct license texts did the Rust half actually yield?
import os
import re
import json
import hashlib
import collections

QT = r"C:/Users/baiyl3/dev/qt/6.10.1/msvc2022_64"
KITS = r"C:/Program Files (x86)/Windows Kits/10"
DIST = r"C:/Users/baiyl3/Documents/projects/ai/ai-flaconshot-v2/spike/p7-package/dist-ci"
LICS = r"C:/Users/baiyl3/Documents/projects/ai/ai-flaconshot-v2/spike/p9-licenses"
VS = r"C:/Program Files/Microsoft Visual Studio/2022"

# Token matcher: "Examples"/"Templates" must not match mpl, "submit" must not match mit,
# but "LICENSE-GPL3", "LGPL-3.0.txt", "sdk_license.rtf" and "REUSE.toml" must match.
EXACT = {"gpl", "lgpl", "agpl", "fdl", "mpl", "mit", "bsd", "ofl", "zlib", "apache",
         "isc", "eula", "unlicense", "copying", "copyright", "reuse", "notice", "spdx"}
PREFIX = ("licen", "copyri", "gpl", "lgpl", "agpl", "mpl", "fdl", "bsd", "apache",
          "ofl", "zlib", "unlicense", "eula", "mit-", "attorni")


def name_match(fn):
    stem = os.path.splitext(fn)[0]
    toks = [t for t in re.split(r"[^a-z0-9]+", stem.lower()) if t]
    for t in toks:
        if t in EXACT or any(t.startswith(p) for p in PREFIX):
            return True
    return False

ev = os.path.join(LICS, "evidence")
os.makedirs(ev, exist_ok=True)


def scan(root, cap=400):
    hits = []
    nfiles = 0
    nbytes = 0
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in (".git", "target")]
        for fn in filenames:
            nfiles += 1
            if name_match(fn):
                full = os.path.join(dirpath, fn)
                try:
                    sz = os.path.getsize(full)
                except OSError:
                    sz = -1
                nbytes += sz
                hits.append((os.path.relpath(full, root).replace("\\", "/"), sz))
    hits.sort(key=lambda h: (-h[1], h[0]))
    return nfiles, nbytes, hits[:cap], len(hits)


lines = []
lines.append("=== Qt install tree: %s ===" % QT)
n, b, hits, total = scan(QT)
lines.append("files scanned: %d ; license-named files: %d (%d bytes)" % (n, total, b))
for h, sz in hits:
    lines.append("  %9d  %s" % (sz, h))

lines.append("")
lines.append("=== what aqt actually installed for licensing purposes ===")
for sub in ("doc", "sbom", "mkspecs", "share"):
    d = os.path.join(QT, sub)
    if os.path.isdir(d):
        cnt = sum(len(f) for _, _, f in os.walk(d))
        size = 0
        for dp, _, fs in os.walk(d):
            for f in fs:
                try:
                    size += os.path.getsize(os.path.join(dp, f))
                except OSError:
                    pass
        lines.append("  %-9s %6d files %10.2f MiB" % (sub, cnt, size / 1048576.0))
        listing = []
        for dp, dns, fs in os.walk(d):
            for f in fs:
                listing.append(os.path.relpath(os.path.join(dp, f), d).replace("\\", "/"))
        listing.sort()
        for x in listing[:25]:
            lines.append("      " + x)
        if len(listing) > 25:
            lines.append("      ... (%d more)" % (len(listing) - 25))

sbom_lic = collections.Counter()
sbom_decl = collections.Counter()
sbom_pkgs = 0
sdir = os.path.join(QT, "sbom")
if os.path.isdir(sdir):
    for fn in sorted(os.listdir(sdir)):
        if not fn.endswith(".json"):
            continue
        try:
            j = json.load(open(os.path.join(sdir, fn), encoding="utf-8"))
        except Exception as e:
            lines.append("sbom parse fail %s: %s" % (fn, e))
            continue
        pgs = j.get("packages", [])
        sbom_pkgs += len(pgs)
        lines.append("  %s: spdxId=%s packages=%d version=%s" % (
            fn, j.get("SPDXID"), len(pgs), j.get("documentNamespace", "")[-24:]))
        for pg in pgs:
            sbom_lic[str(pg.get("licenseConcluded"))] += 1
            sbom_decl[str(pg.get("licenseDeclared"))] += 1
    lines.append("")
    lines.append("=== sbom SPDX: total package records across the 7 modules ===")
    lines.append("  package records: %d" % sbom_pkgs)
    lines.append("  licenseConcluded tally:")
    for k, v in sbom_lic.most_common(40):
        lines.append("    %5d  %s" % (v, k))
    lines.append("  licenseDeclared tally:")
    for k, v in sbom_decl.most_common(40):
        lines.append("    %5d  %s" % (v, k))

lines.append("")
lines.append("=== REUSE / LICENSES material inside the Qt tree ===")
for dp, dns, fs in os.walk(QT):
    base = os.path.basename(dp).lower()
    if base in ("licenses", "license", "licenses.txt"):
        lines.append("  DIR %s (%d files)" % (dp.replace(QT + "/", ""), len(fs)))
        for f in sorted(fs)[:12]:
            lines.append("      " + f)
    for f in fs:
        if f.lower() in ("reuse.toml", "license.md", "license.txt", "lgpl-3.txt",
                        "commercial-license.txt", "qt license.txt"):
            full = os.path.join(dp, f)
            lines.append("  FILE %s  %d bytes" % (
                os.path.relpath(full, QT).replace("\\", "/"), os.path.getsize(full)))

lines.append("")
lines.append("=== Windows Kits 10 Licenses: %s ===" % os.path.join(KITS, "Licenses"))
lic_root = os.path.join(KITS, "Licenses")
if os.path.isdir(lic_root):
    for dp, dns, fs in os.walk(lic_root):
        for f in sorted(fs):
            full = os.path.join(dp, f)
            lines.append("  %9d  %s" % (os.path.getsize(full),
                                        os.path.relpath(full, lic_root).replace("\\", "/")))
else:
    lines.append("  (absent)")

lines.append("")
lines.append("=== MSVC redistributable installer present locally? ===")
import glob as _glob
found = 0
roots = [r"C:/Program Files/Microsoft Visual Studio",
         r"C:/Program Files (x86)/Microsoft Visual Studio"]
for root in roots:
    for pat in ("*/VC/Redist/MSVC/*/x64/vc_redist.x64.exe",
                "**/vc_redist.x64.exe"):
        for exe in sorted(_glob.glob(os.path.join(root, pat), recursive=True)):
            if os.path.isfile(exe):
                found += 1
                lines.append("  %10d  %s" % (os.path.getsize(exe), exe))
        if found:
            break
if not found:
    lines.append("  none found under %s" % ", ".join(roots))
    lines.append("  VS roots present? " + ", ".join(
        "%s=%s" % (r, os.path.isdir(r)) for r in roots))

lines.append("")
lines.append("=== pinned package binary inventory (%s) ===" % DIST)
qtdll = []
plugins = []
other = []
for dp, dns, fs in os.walk(DIST):
    for f in fs:
        if not f.lower().endswith(".dll"):
            continue
        full = os.path.join(dp, f)
        rel = os.path.relpath(full, DIST).replace("\\", "/")
        sz = os.path.getsize(full)
        if rel.startswith("plugins/"):
            plugins.append((rel, sz))
        elif f.startswith("Qt6") or f.startswith("Qt"):
            qtdll.append((rel, sz))
        else:
            other.append((rel, sz))
qtdll.sort(); plugins.sort(); other.sort()
lines.append("Qt module DLLs: %d (%.2f MiB)" % (len(qtdll), sum(s for _, s in qtdll) / 1048576.0))
for r, s in qtdll:
    lines.append("  %9d  %s" % (s, r))
lines.append("plugin DLLs: %d (%.2f MiB)" % (len(plugins), sum(s for _, s in plugins) / 1048576.0))
for r, s in plugins:
    lines.append("  %9d  %s" % (s, r))
lines.append("other DLLs: %d (%.2f MiB)" % (len(other), sum(s for _, s in other) / 1048576.0))
for r, s in other:
    lines.append("  %9d  %s" % (s, r))

lines.append("")
lines.append("=== collected Rust license texts: dedupe ===")
d = os.path.join(LICS, "licenses")
by_hash = collections.defaultdict(list)
for fn in sorted(os.listdir(d)):
    full = os.path.join(d, fn)
    h = hashlib.sha256(open(full, "rb").read()).hexdigest()[:16]
    by_hash[h].append(fn)
lines.append("files: %d ; distinct by sha256: %d" % (len(os.listdir(d)), len(by_hash)))
for h, files in sorted(by_hash.items(), key=lambda kv: -len(kv[1])):
    lines.append("  %2d copy/copies  %s  e.g. %s" % (len(files), h, files[0]))

with open(os.path.join(ev, "qt-dist-scan.txt"), "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(lines) + "\n")
print("\n".join(lines[:12]))
print("...")
print("written: evidence/qt-dist-scan.txt (%d lines)" % len(lines))
