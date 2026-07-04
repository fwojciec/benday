# Demo recording for the README. Regenerate with:
#   cargo build --release && vhs assets/demo.tape

Output assets/demo.gif

Set FontSize 14
Set Width 780
Set Height 440
Set Padding 20
Set Theme "TokyoNight"

Type "export PATH=$PWD/target/release:$PATH && clear"
Enter

Type "benday --spec-file examples/bar.json"
Sleep 600ms
Enter
Sleep 3.5s

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
Type `echo '{"data":{"values":[{"a":1,"b":2}]},"mark":"donut","encoding":{"x":{"field":"a"},"y":{"field":"b"}}}' | benday`
Sleep 600ms
Enter
Sleep 4.5s

Type "clear"
Enter

Type "benday --spec-file examples/area.json --theme lichtenstein"
Sleep 600ms
Enter
Sleep 4.5s
