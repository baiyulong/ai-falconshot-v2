#!/usr/bin/env python3
# P9 license probe (spike only, not product code).
# Answers one question with measurement: what license material does a shipped
# Falconshot package actually need, and can we get it from this machine offline?
import json
import os
import re
import subprocess
import sys
import glob
import shutil
import collections

HERE = os.path.dirname(os.path.abspath(__file__))
SPIKE = os.path.dirname(HERE)
CRATE = os.path.join(SPIKE, "hello-cxxqt")
OUT = os.path.join(HERE, "licenses")
EV = os.path.join(HERE, "evidence")
DIST = os.path.join(SPIKE, "p7-package", "dist-ci")
QT = os.environ.get("QT_DIR", r"C:/Users/baiyl3/dev/qt/6.10.1/msvc2022_64")
PLATFORM = "x86_64-pc-windows-msvc"

os.makedirs(OUT, exist_ok=True)
os.makedirs(EV, exist_ok=True)


def run_meta():
    p = subprocess.run(
        ["cargo", "metadata", "--offline", "--format-version", "1",
         "--filter-platform", PLATFORM],
        cwd=CRATE, capture_output=True, text=True, encoding="utf-8")
    if p.returncode != 0:
        print("cargo metadata failed rc=%s" % p.returncode)
        print(p.stderr[:2000])
        sys.exit(1)
    return json.loads(p.stdout)


def closures(meta):
    root = meta["resolve"]["root"]
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}

    # `deps[].kind` is deprecated/absent for normal edges; `dep_kinds[]` is the
    # authoritative per-edge record (kind == None means a normal dependency).
    def edge_kinds(dep):
        dks = dep.get("dep_kinds")
        if not dks:
            return [dep.get("kind")]
        return [d.get("kind") for d in dks]

    def reach(keep):
        seen = {root}
        stack = [root]
        while stack:
            cur = stack.pop()
            for dep in nodes.get(cur, {}).get("deps", []):
                ks = edge_kinds(dep)
                if keep(ks) and dep["pkg"] not in seen:
                    seen.add(dep["pkg"])
                    stack.append(dep["pkg"])
        return seen - {root}

    runtime = reach(lambda ks: None in ks)
    used_at_build_time = reach(lambda ks: (None in ks) or ("build" in ks))
    buildonly = used_at_build_time - runtime
    dev = reach(lambda ks: "dev" in ks) - runtime - buildonly
    return {"runtime": runtime, "build": buildonly, "dev": dev}


SRC_DIRS = glob.glob(os.path.join(os.path.expanduser("~"), ".cargo",
                                 "registry", "src", "*"))
LICENSE_RE = re.compile(r"^(licen[cs]e|copying|copyright|unlicense|ofl|mit|apache|bsd|gpl|lgpl|agpl|eula|autho)", re.I)


def find_src_dir(name, version):
    cand = "%s-%s" % (name, version)
    for d in SRC_DIRS:
        p = os.path.join(d, cand)
        if os.path.isdir(p):
            return p
    return None


def license_files(src):
    out = []
    if not src:
        return out
    for d in ("", "LICENSES", "licenses", "license"):
        full_dir = os.path.join(src, d) if d else src
        if not os.path.isdir(full_dir):
            continue
        for fn in sorted(os.listdir(full_dir)):
            full = os.path.join(full_dir, fn)
            if not os.path.isfile(full):
                continue
            if LICENSE_RE.match(fn) and os.path.getsize(full) > 40:
                out.append(fn if not d else d + "/" + fn)
    return out


FAMILY = [
    ("AGPL", "strong"), ("GPL", "strong"), ("LGPL", "weak"), ("MPL", "weak"),
    ("CC-BY-NC", "restrictive"), ("NonStandard", "unknown"),
    ("MIT", "permissive"), ("Apache", "permissive"), ("BSD", "permissive"),
    ("ISC", "permissive"), ("Zlib", "permissive"), ("Unlicense", "permissive"),
    ("CC0", "permissive"), ("Unicode", "permissive"), ("BSL", "source-available"),
    ("OpenSSL", "weak"), ("Unicode-DFS", "permissive"),
]


def classify(expr):
    if not expr:
        return ("none", "unknown")
    toks = set()
    for pat, kind in FAMILY:
        if re.search(r"\b" + pat, expr, re.I):
            toks.add((pat, kind))
    if not toks:
        return (expr, "unknown")
    order = ["restrictive", "strong", "unknown", "source-available", "weak", "permissive"]
    worst = sorted(toks, key=lambda t: order.index(t[1]))[0]
    return (worst[0], worst[1])


def main():
    meta = run_meta()
    pkgs = {p["id"]: p for p in meta["packages"]}
    kinds = closures(meta)
    print("workspace root members:", len(meta["workspace_default_members"]))
    print("packages in filtered graph:", len(meta["packages"]))
    print("runtime closure:", len(kinds["runtime"]))
    print("build closure:", len(kinds["build"]))
    print("dev closure:", len(kinds["dev"]))

    rows = []
    fam_count = collections.Counter()
    expr_count = collections.Counter()
    noclaw = []
    notext = []
    copied = 0
    copied_rt = 0

    for bucket in ("runtime", "build", "dev"):
        for pid in sorted(kinds[bucket]):
            p = pkgs[pid]
            src = find_src_dir(p["name"], p["version"])
            lf = license_files(src)
            expr = p.get("license") or ""
            fam, kind = classify(expr)
            rows.append("\t".join([
                bucket, p["name"], p["version"], expr or "(no license field)",
                fam, kind, "yes" if src else "NO-SRC", ",".join(lf) or "-",
                p.get("description", "").replace("\t", " ").replace("\n", " ")[:70]
            ]))
            if bucket == "runtime":
                fam_count[kind + "/" + fam] += 1
                expr_count[expr or "(none)"] += 1
                if not expr:
                    noclaw.append(p["name"] + " " + p["version"])
                if not lf:
                    notext.append((p["name"], p["version"], expr, src))
            for fn in lf:
                dst = os.path.join(OUT, "%s-%s-%s" % (p["name"], p["version"], fn.replace("/", "_")))
                try:
                    shutil.copyfile(os.path.join(src, fn), dst)
                    copied += 1
                    if bucket == "runtime":
                        copied_rt += 1
                except Exception as e:
                    print("copy fail", dst, e)

    with open(os.path.join(EV, "rust-deps.tsv"), "w", encoding="utf-8", newline="\n") as f:
        f.write("bucket\tcrate\tversion\tlicense-spdx\tfamily\tclass\tsrc-in-cache\tlicense-files\tdescription\n")
        f.write("\n".join(rows) + "\n")

    rt = [pid for pid in kinds["runtime"]]
    with open(os.path.join(EV, "rust-summary.txt"), "w", encoding="utf-8", newline="\n") as f:
        f.write("filter-platform: %s\n" % PLATFORM)
        f.write("runtime crates: %d\n" % len(rt))
        f.write("license files copied: %d\n" % copied)
        f.write("distinct SPDX expressions in runtime closure:\n")
        for k, v in expr_count.most_common():
            f.write("  %2d  %s\n" % (v, k))
        f.write("class breakdown (runtime):\n")
        for k, v in sorted(fam_count.items()):
            f.write("  %2d  %s\n" % (v, k))
        f.write("crates with no license field (%d):\n" % len(noclaw))
        for x in noclaw:
            f.write("  " + x + "\n")
        f.write("runtime crates with NO license text file found (%d):\n" % len(notext))
        for n, v, e, s in notext:
            f.write("  %s %s  license=%s  src=%s\n" % (n, v, e or "-", s or "missing"))

    print("license files copied:", copied)
    print("runtime crates lacking a license field:", len(noclaw))
    print("runtime crates lacking a license TEXT:", len(notext))
    for n, v, e, s in notext[:20]:
        print("   ", n, v, "|", e or "(none)", "|", "no-src" if not s else "src has no matching file")


main()
