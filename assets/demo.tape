# Demo recording for the README. Regenerate with:
#   cargo build --release && vhs assets/demo.tape

Output assets/demo.gif

Set FontSize 14
Set Width 780
Set Height 520
Set Padding 20
Set Theme "TokyoNight"

Type "export PATH=$PWD/target/release:$PATH && clear"
Enter

Type "# the primary flow: query results piped in, spec as a flag"
Enter
Sleep 500ms
Type@8ms `echo '{"columns":[{"name":"day","type":"STRING"},{"name":"signups","type":"INT64"}],"rows":[["mon",32],["tue",78],["wed",51],["thu",94],["fri",67]]}' | benday --spec '{"title":"weekly signups","mark":"bar","encoding":{"x":{"field":"day"},"y":{"field":"signups"}}}'`
Sleep 600ms
Enter
Sleep 4s

Type "clear"
Enter

Type "benday --spec-file examples/lines.json"
Sleep 600ms
Enter
Sleep 3.5s

Type "clear"
Enter

Type "# bad specs fail loudly -- the error carries the fix:"
Enter
Sleep 500ms
Type@8ms `echo '{"data":{"values":[{"a":1,"b":2}]},"mark":"donut","encoding":{"x":{"field":"a"},"y":{"field":"b"}}}' | benday`
Sleep 600ms
Enter
Sleep 4.5s

Type "clear"
Enter

Type "benday --spec-file examples/area.json --theme lichtenstein"
Sleep 600ms
Enter
Sleep 4.5s
