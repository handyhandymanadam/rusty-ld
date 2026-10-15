#!/usr/bin/env bash
. $(dirname $0)/common.inc

# --size-report tells how many bytes each input file and output section
# contributes, in text or JSON.

cat <<EOF2 | $CC -c -o $t/big.o -xc -
char big_data[4000] = {1};
int big_fn(int x) { return x * 3; }
EOF2

cat <<EOF2 | $CC -c -o $t/small.o -xc -
extern char big_data[];
int big_fn(int);
int main() { return big_fn(big_data[0]); }
EOF2

# Text on stdout. The larger file comes first.
$CC -B. -o $t/exe1 $t/big.o $t/small.o -Wl,--size-report > $t/log1
grep -F 'Output file: ' $t/log1
grep -E '^ +[0-9]+  \.text  \([0-9]+ input sections\)' $t/log1
grep -A3 'By input file' $t/log1 | grep -E '^ +[0-9]+  .*big\.o  \[\.data 4000'

# JSON to a file, picked by the .json extension.
$CC -B. -o $t/exe2 $t/big.o $t/small.o -Wl,--size-report=$t/report.json
grep -E '^\{"schema":1,"type":"size-report"' $t/report.json
grep -E '"file":"[^"]*big\.o","size":[0-9]+,"sections":\[\{"name":"\.data","size":4000\}' $t/report.json
grep -F '"name":".text"' $t/report.json

# The report does not change the output file.
$CC -B. -o $t/exe3 $t/big.o $t/small.o
cmp $t/exe2 $t/exe3

if command -v python3 > /dev/null; then
  python3 -c '
import json, sys
d = json.load(open(sys.argv[1]))
assert d["files_omitted"] == 0 and d["omitted_size"] == 0
assert d["output_size"] > sum(f["size"] for f in d["files"])
assert [f["size"] for f in d["files"]] == sorted((f["size"] for f in d["files"]), reverse=True)
' $t/report.json
fi

# On standard output the JSON lists only the 30 largest files and counts
# the rest.
for i in $(seq 40); do
  echo "int fn$i(int x) { return x * $i; }" | $CC -c -o $t/many$i.o -xc -
done
cat <<EOF2 | $CC -c -o $t/main.o -xc -
int main() { return 0; }
EOF2
$CC -B. -o $t/exe4 $t/main.o $t/many?.o $t/many??.o -Wl,--diagnostics-format=json,--size-report > $t/log4
python3 -c '
import json, sys
d = json.loads(open(sys.argv[1]).readline())
assert len(d["files"]) == 30, len(d["files"])
assert d["files_omitted"] >= 11 and d["omitted_size"] > 0, d
' $t/log4
