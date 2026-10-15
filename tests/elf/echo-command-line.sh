#!/usr/bin/env bash
. $(dirname $0)/common.inc

# --echo-command-line prints the command line and what each response file
# expands to.

echo 'int main() { return 0; }' | $CC -c -o $t/a.o -xc -
printf -- '--echo-command-line\n"-z" nodelete\n' > $t/args.rsp

$CC -B. -o $t/exe $t/a.o -Wl,@$t/args.rsp > $t/log1
grep -E '^command line: .* -o .*a\.o ' $t/log1
grep -E "^@.*args\.rsp expands to: --echo-command-line -z nodelete$" $t/log1
grep -E '^working directory: /' $t/log1

# JSON
$CC -B. -o $t/exe $t/a.o -Wl,--diagnostics-format=json,--echo-command-line > $t/log2
grep -E '^\{"schema":1,"type":"command-line","cwd":"/[^"]*","args":\["[^"]*(ld|mold)"' $t/log2
$CC -B. -o $t/exe $t/a.o -Wl,--diagnostics-format=json,@$t/args.rsp > $t/log3
grep -F '"response_files":[{"path":"@' $t/log3
grep -F '"args":["--echo-command-line","-z","nodelete"]' $t/log3

# Off by default
$CC -B. -o $t/exe $t/a.o > $t/log4
[ ! -s $t/log4 ]

# A long command line is cut in JSON only: the first 20 and the last 5
# arguments, with the counts.
args=$(for i in $(seq 40); do printf -- '-Wl,-rpath,/d%s ' $i; done)
$CC -B. -o $t/exe5 $t/a.o $args -Wl,--diagnostics-format=json,--echo-command-line > $t/log5
python3 -c '
import json, sys
r = json.loads(open(sys.argv[1]).readline())
assert r["args_total"] > 25 and r["args_omitted"] == r["args_total"] - 25, r
assert len(r["args"]) == 25 and r["args"][-1].endswith("crtn.o"), r
assert not any(a == "/d20" for a in r["args"]), r
' $t/log5
$CC -B. -o $t/exe5 $t/a.o $args -Wl,--echo-command-line > $t/log6
grep -F -- '-rpath /d20 ' $t/log6
grep -F -- '-rpath /d40 ' $t/log6
