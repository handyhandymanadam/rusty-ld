#!/usr/bin/env bash
. $(dirname $0)/common.inc

# The undefined-symbol record in agent mode: source lines of the references
# (when the object has line tables), notes without repeats, hints only for
# symbols that are defined and libraries that are not on the link yet, one
# hint text in the rendered and the JSON form, and no near misses from
# unrelated system libraries.

mkdir -p $t/lib

cat <<EOF2 > $t/m1.c
int missing_fn(void);
double cos_like(double);
int a(void) { return missing_fn(); }
int b(void) { return missing_fn(); }
int c(void) { return (int)cos_like(1.0); }
int main(void) { return a() + b() + c(); }
EOF2
cat <<EOF2 > $t/m2.c
int missin_fn(int);
int usd(void);
int f(void) { return missin_fn(1) + usd(); }
EOF2
cat <<EOF2 > $t/d.c
int missin_fm(int x) { return x; }
EOF2
(cd $t && $CC -g -O0 -fno-builtin -c m1.c m2.c d.c)
# The same without line tables.
$CC -O0 -fno-builtin -c -o $t/m1n.o $t/m1.c

# Libraries: a versioned alias next to the plain name, and a static library
# whose shared twin is already on the link.
echo 'double cos_like(double x) { return x; }' | $CC -fPIC -shared -o $t/lib/libcosl.so -xc -
cp $t/lib/libcosl.so $t/lib/libcosl-2.5.so
echo 'int gadget_a(void) { return 1; }' | $CC -fPIC -shared -o $t/lib/libgadget.so -xc -
echo 'int gadget_a(void) { return 1; } int gadget_b(void) { return 2; }' | $CC -c -o $t/gadget.o -xc -
rm -f $t/lib/libgadget.a; ar rcs $t/lib/libgadget.a $t/gadget.o
cat <<EOF2 | $CC -g -c -o $t/g.o -xc -
int gadget_a(void);
int gadget_b(void);
int main(void) { return gadget_a() + gadget_b(); }
EOF2

run() { RUSTY_AGENT=1 not $CC -B. -o $t/exe "$@" 2> $t/err; }

# 1. Source lines, deduplicated, no repeated notes at normal.
run $t/m1.o $t/m2.o $t/d.o -L$t/lib
python3 -I - $t/err $t/m1.c <<'EOF2'
import json, os, sys
recs = [json.loads(l) for l in open(sys.argv[1]) if l.startswith('{')]
by = {r['message'].split(': ')[1]: r for r in recs if r['type'] == 'diagnostic'}
r = by['missing_fn']
assert list(r)[:5] == ['schema', 'type', 'code', 'severity', 'message'], list(r)
for gone in ('rendered', 'files', 'symbols', 'notes'):
    assert gone not in r, (gone, r)
locs = r['locations']
assert [(l['function'], l['line']) for l in locs] == [('a', 3), ('b', 4)], locs
assert all(os.path.basename(l['object']) == 'm1.o' and os.path.basename(l['file']) == 'm1.c' for l in locs), locs
assert 'references_omitted' not in r, r
# missin_fn is also undefined, so it is not offered for missing_fn.
assert not any('missin_fn`' in x['message'] for x in r.get('suggestions', [])), r
# Size budget for one undefined-symbol record at normal.
assert len(json.dumps(r, separators=(',', ':'))) < 600, len(json.dumps(r))
# 3. A near miss must be a symbol that is defined: missin_fn gets missin_fm,
#    not the undefined missing_fn.
s = by['missin_fn']['suggestions']
assert len(s) == 1 and s[0]['message'].startswith('did you mean `missin_fm`? (defined in ') and s[0]['message'].endswith('d.o)'), s
# A short name is not matched against names in system libraries.
assert 'suggestions' not in by['usd'], by['usd']
EOF2

# 5. Without line tables the line is left out and the source name is kept.
run $t/m1n.o -L$t/lib
python3 -I - $t/err <<'EOF2'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1]) if l.startswith('{')]
r = [r for r in recs if r['message'] == 'undefined symbol: missing_fn'][0]
assert all('line' not in l for l in r['locations']), r
assert all(l['file'].endswith('m1.c') for l in r['locations']), r
EOF2

# 6. At full the notes are there, with the repeated "referenced by" line once
#    and counted; the rendered text and the files and symbols are there too.
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=full not $CC -B. -o $t/exe $t/m1.o -L$t/lib 2> $t/err
python3 -I - $t/err <<'EOF2'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1]) if l.startswith('{')]
r = [r for r in recs if r['message'] == 'undefined symbol: missing_fn'][0]
for k in ('rendered', 'files', 'symbols', 'notes', 'suggestions', 'locations'):
    assert k in r, k
refs = [n for n in r['notes'] if n.startswith('referenced by')]
assert refs == ['referenced by m1.c (2 references)'], r['notes']
assert r['rendered'].count('referenced by') == 2, r['rendered']   # the text is as it was
EOF2

# 4. Library hints: the stable name, not the versioned alias; and no hint for a
#    library that is on the link (the shared twin of a static library).
run $t/m1.o -L$t/lib
python3 -I - $t/err <<'EOF2'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1]) if l.startswith('{')]
r = [r for r in recs if r['message'] == 'undefined symbol: cos_like'][0]
opts = [s.get('option') for s in r['suggestions']]
assert opts == ['-lcosl'], opts
EOF2
run $t/g.o -L$t/lib -lgadget
python3 -I - $t/err <<'EOF2'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1]) if l.startswith('{')]
r = [r for r in recs if r['message'] == 'undefined symbol: gadget_b'][0]
assert not any(x.get('option') for x in r.get('suggestions', [])), r   # -lgadget is on the link already
EOF2

# A4: the hint for a library outside the -L list says the same thing in the
# rendered text and in the suggestion. Linking by hand, with no -L option, puts
# the system's libm outside the list.
cat <<EOF2 | $CC -c -o $t/cu.o -xc -
double cos(double);
int main(void) { return (int)cos(1.0); }
EOF2
RUSTY_AGENT=1 RUSTY_AGENT_DETAIL=full not ./mold -o $t/exe $t/cu.o 2> $t/err
python3 -I - $t/err <<'EOF2'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1]) if l.startswith('{')]
r = [r for r in recs if r['message'] == 'undefined symbol: cos'][0]
hint = [l[len('>>> hint: '):] for l in r['rendered'].splitlines() if l.startswith('>>> hint: ')]
assert hint and hint == [s['message'] for s in r['suggestions']], (hint, r['suggestions'])
assert any('-L' in (s.get('option') or '') for s in r['suggestions']), r['suggestions']
EOF2

# A4: an archive on the link whose member nothing pulled in still gives a near miss.
echo 'int libfn(void) { return 3; }' | $CC -c -o $t/lib1.o -xc -
rm -f $t/libx.a; ar rcs $t/libx.a $t/lib1.o
cat <<EOF2 | $CC -c -o $t/use.o -xc -
int libfnn(void);
int main(void) { return libfnn(); }
EOF2
run $t/use.o $t/libx.a
python3 -I - $t/err <<'EOF2'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1]) if l.startswith('{')]
r = [r for r in recs if r['message'] == 'undefined symbol: libfnn'][0]
assert [s['message'].startswith('did you mean `libfn`?') and s['message'].endswith('libx.a)') for s in r['suggestions']] == [True], r
EOF2

# Plain text output of --explain-undefined (no JSON) is unchanged by all this.
not $CC -B. -o $t/exe $t/m1.o $t/m2.o $t/d.o -Wl,--explain-undefined >& $t/plain
grep -F '>>> referenced by m1.c' $t/plain
grep -F 'did you mean `missin_fm`' $t/plain
