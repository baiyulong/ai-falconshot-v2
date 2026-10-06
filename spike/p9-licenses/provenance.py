import os, re, struct, hashlib, json, sys

DIST = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', 'p7-package', 'dist-ci'))
QTROOT = 'C:/Users/baiyl3/dev/qt/6.10.1/msvc2022_64'
SYS32 = os.path.join(os.environ.get('SystemRoot', 'C:/Windows'), 'System32')
OUT = os.path.join(os.path.dirname(__file__), 'evidence', 'binary-provenance.txt')

lines = []
def say(s=''):
    lines.append(s)

def md5(p):
    h = hashlib.md5()
    with open(p, 'rb') as f:
        for b in iter(lambda: f.read(1 << 20), b''):
            h.update(b)
    return h.hexdigest()

dlls = []
for dp, dn, fn in os.walk(DIST):
    for f in fn:
        if f.lower().endswith('.dll'):
            dlls.append(os.path.relpath(os.path.join(dp, f), DIST).replace('\\', '/'))
dlls.sort()
say('=== binary provenance: dist-ci DLLs vs %s ===' % SYS32)
say('dist root   : %s' % DIST)
say('dll count   : %d' % len(dlls))
say('')

same, diff, none_ = [], [], []
for rel in dlls:
    full = os.path.join(DIST, rel.replace('/', os.sep))
    twin = os.path.join(SYS32, os.path.basename(rel))
    if os.path.isfile(twin):
        a, b = md5(full), md5(twin)
        sa, sb = os.path.getsize(full), os.path.getsize(twin)
        (same if a == b else diff).append((rel, sa, sb, a, b, os.path.getmtime(full)))
    else:
        none_.append((rel, os.path.getsize(full)))

say('bucket A  byte-identical to a System32 same-name file : %d' % len(same))
for rel, sa, sb, a, b, mt in same:
    say('   %-16s dist=%d sys=%d md5=%s' % (rel, sa, sb, a))
say('bucket B  same name in System32, different bytes      : %d' % len(diff))
for rel, sa, sb, a, b, mt in diff:
    say('   %-24s dist=%d md5=%s | sys=%d md5=%s' % (rel, sa, a, sb, b))
say('bucket C  no same-name file in System32               : %d' % len(none_))
say('')

say('=== icuuc.dll: does the Qt install own it? ===')
hits = []
for dp, dn, fn in os.walk(QTROOT):
    for f in fn:
        if 'icu' in f.lower():
            hits.append(os.path.relpath(os.path.join(dp, f), QTROOT).replace('\\', '/'))
hits.sort()
say('files under Qt root whose name contains "icu": %d' % len(hits))
for h in hits[:20]:
    say('   %s' % h)
say('')

icu = os.path.join(DIST, 'icuuc.dll')
say('dist icuuc.dll exists=%s size=%d md5=%s' % (os.path.isfile(icu),
    os.path.getsize(icu) if os.path.isfile(icu) else -1,
    md5(icu) if os.path.isfile(icu) else '-'))
sysicu = os.path.join(SYS32, 'icuuc.dll')
say('system icuuc.dll exists=%s size=%d md5=%s' % (os.path.isfile(sysicu),
    os.path.getsize(sysicu) if os.path.isfile(sysicu) else -1,
    md5(sysicu) if os.path.isfile(sysicu) else '-'))
say('')


def imports(path):
    with open(path, 'rb') as f:
        data = f.read()
    pe = struct.unpack_from('<I', data, 0x3c)[0]
    if data[pe:pe + 4] != b'PE\x00\x00':
        return None, 0
    nsec, = struct.unpack_from('<H', data, pe + 6)
    optsize, = struct.unpack_from('<H', data, pe + 20)
    opt = pe + 24
    magic, = struct.unpack_from('<H', data, opt)
    dd = opt + (112 if magic == 0x20b else 96)
    imp_rva, imp_sz = struct.unpack_from('<II', data, dd + 8)
    secs = []
    p = opt + optsize
    for i in range(nsec):
        o = p + i * 40
        name = data[o:o + 8].rstrip(b'\0').decode('ascii', 'replace')
        vsz, vaddr, rawsz, praw = struct.unpack_from('<IIII', data, o + 8)
        secs.append((name, vaddr, vsz, praw, rawsz))

    def off_of(rva):
        for n, va, vsz, praw, rawsz in secs:
            if va <= rva < va + max(vsz, rawsz):
                return praw + (rva - va)
        return None

    out = []
    o = off_of(imp_rva)
    while o:
        namerva, = struct.unpack_from('<I', data, o + 12)
        orig, = struct.unpack_from('<I', data, o)
        if namerva == 0 and orig == 0:
            break
        no = off_of(namerva)
        if no is None:
            break
        e = data.index(b'\0', no)
        out.append(data[no:e].decode('ascii', 'replace'))
        o += 20
    return out, magic


say('=== PE import tables (hand-parsed, data directory index 1) ===')
for mod in ('Qt6Core.dll', 'Qt6Gui.dll', 'Qt6Quick.dll', 'icuuc.dll'):
    fp = os.path.join(DIST, mod)
    if not os.path.isfile(fp):
        say('%-14s ABSENT from dist' % mod)
        continue
    imps, magic = imports(fp)
    if imps is None:
        say('%-14s not a PE' % mod)
        continue
    say('%-14s pe32plus=%s imports=%d' % (mod, magic == 0x20b, len(imps)))
    say('   %s' % ' '.join(sorted(imps)))
    for need in ('icuuc.dll', 'MSVCP140.dll', 'MSVCP140_1.dll', 'MSVCP140_2.dll',
                 'VCRUNTIME140.dll', 'VCRUNTIME140_1.dll'):
        if need in [i.lower() for i in imps]:
            say('   -> requires %s' % need)
    crt = [i for i in imps if i.lower().startswith('api-ms-win-crt-')]
    if crt:
        say('   -> api-ms-win-crt-* count = %d (%s)' % (len(crt), ' '.join(sorted(crt))))
    say('')

say('=== is any of these needed DLLs shipped in the package? ===')
have = set(os.path.basename(d).lower() for d in dlls)
for need in ('icuuc.dll', 'MSVCP140.dll', 'MSVCP140_1.dll', 'MSVCP140_2.dll',
             'VCRUNTIME140.dll', 'VCRUNTIME140_1.dll'):
    say('   %-20s in package = %s' % (need, need in have))
say('')

say('=== icuuc.dll in the Qt SBOM (all 7 spdx json files)? ===')
sbomdir = os.path.join(QTROOT, 'sbom')
tot = 0
icu_rec = 0
for dp, dn, fn in os.walk(sbomdir):
    for f in fn:
        if f.endswith('.spdx.json'):
            try:
                doc = json.load(open(os.path.join(dp, f), encoding='utf-8'))
            except Exception as ex:
                say('   parse fail %s: %s' % (f, ex))
                continue
            pk = doc.get('packages', [])
            tot += len(pk)
            for p in pk:
                blob = json.dumps(p).lower()
                if 'icu' in blob:
                    icu_rec += 1
                    say('   icu mention in %s -> %s' % (f, p.get('name')))
say('   total package records = %d ; icu mentions = %d' % (tot, icu_rec))
say('')

say('=== vc_redist candidates on this machine ===')
for root in ('C:/Program Files/Microsoft Visual Studio/2022',
             'C:/Program Files (x86)/Microsoft Visual Studio/2022'):
    for dp, dn, fn in os.walk(root):
        for f in fn:
            if f.lower() == 'vc_redist.x64.exe':
                fp = os.path.join(dp, f)
                say('   %d B  %s' % (os.path.getsize(fp), fp))
say('')

say('=== D3Dcompiler_47.dll: dist vs Qt tree vs SDK ===')
d = os.path.join(DIST, 'D3Dcompiler_47.dll')
if os.path.isfile(d):
    say('   dist      %d B md5=%s' % (os.path.getsize(d), md5(d)))
q = os.path.join(QTROOT, 'bin', 'D3Dcompiler_47.dll')
if os.path.isfile(q):
    say('   qt/bin    %d B md5=%s' % (os.path.getsize(q), md5(q)))
else:
    say('   qt/bin    ABSENT')
for dp, dn, fn in os.walk(os.path.join(os.environ.get('ProgramFiles(x86)', 'C:/Program Files (x86)'), 'Windows Kits', '10', 'bin')) if os.path.isdir(os.path.join(os.environ.get('ProgramFiles(x86)', 'C:/Program Files (x86)'), 'Windows Kits', '10', 'bin')) else []:
    for f in fn:
        if f.lower() == 'd3dcompiler_47.dll' and 'x64' in dp:
            fp = os.path.join(dp, f)
            say('   sdk(%s) %d B md5=%s' % (os.path.basename(dp), os.path.getsize(fp), md5(fp)))
if os.path.isfile(sysicu):
    pass
s = os.path.join(SYS32, 'D3Dcompiler_47.dll')
say('   System32  %s' % ('%d B md5=%s' % (os.path.getsize(s), md5(s)) if os.path.isfile(s) else 'ABSENT'))

os.makedirs(os.path.dirname(OUT), exist_ok=True)
open(OUT, 'w', encoding='utf-8', newline='\n').write('\n'.join(lines) + '\n')
print('\n'.join(lines[:12]))
print('...')
print('wrote %s (%d lines)' % (OUT, len(lines)))
