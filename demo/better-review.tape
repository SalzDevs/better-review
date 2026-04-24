Output demo/better-review.gif
Output demo/better-review.mp4

Require bash
Require cargo
Require git
Require ffmpeg
Require ttyd

Set Shell "bash"
Set FontSize 18
Set Width 1480
Set Height 920
Set Padding 24
Set Margin 16
Set MarginFill "#0f1117"
Set BorderRadius 10
Set CursorBlink false
Set TypingSpeed 1ms
Set PlaybackSpeed 1.0
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful

Hide
Type "repo_root=$PWD; demo_repo=$(bash demo/setup-demo-repo.sh); cd $demo_repo && cargo run --manifest-path $repo_root/Cargo.toml"
Enter
Wait+Screen /Enter review/
Show

Sleep 900ms
Enter
Wait+Screen /reviewing files/

Sleep 700ms
Enter
Wait+Screen /inspecting hunks/
Sleep 900ms
Escape
Wait+Screen /reviewing files/

Sleep 600ms
Type "y"
Wait+Screen /\[✓\] src\/config\.rs/

Sleep 650ms
Type "j"
Sleep 500ms
Type "x"
Wait+Screen /\[x\] src\/debug\.rs/

Sleep 650ms
Type "j"
Sleep 500ms
Type "y"
Wait+Screen /\[✓\] src\/lib\.rs/

Sleep 800ms
Type "s"
Wait+Screen /Settings/
Sleep 900ms
Escape
Wait+Screen /reviewing files/

Sleep 600ms
Type "c"
Wait+Screen /Commit prompt active/
Sleep 500ms
Type "feat: review generated changes"
Sleep 350ms
Enter
Wait+Screen /Publish/

Sleep 900ms
Type "j"
Sleep 450ms
Enter
Wait+Screen /reviewing files/
Sleep 800ms
